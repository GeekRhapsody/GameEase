use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

const NM_DEST: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const NM_DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const NM_AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const NM_SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const NM_SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const NM_SETTINGS_CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const NM_DEVICE_TYPE_WIFI: u32 = 2;
const NM_802_11_AP_FLAGS_PRIVACY: u32 = 0x1;
const SCAN_TIMEOUT: Duration = Duration::from_secs(8);
const SCAN_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Nearby Wi-Fi network details from NetworkManager.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WifiNetwork {
    /// Human-readable SSID.
    pub ssid: String,
    /// Signal strength percentage.
    pub strength: u8,
    /// Whether the access point requires credentials.
    pub secured: bool,
    /// Whether this SSID is currently active.
    pub connected: bool,
}

/// Messages emitted by the Wi-Fi worker for the GTK side menu.
#[derive(Debug)]
pub enum WifiEvent {
    /// A scan started.
    ScanStarted,
    /// Scan completed with nearby networks.
    ScanFinished(Vec<WifiNetwork>),
    /// A connection attempt started for an SSID.
    ConnectStarted(String),
    /// A password is required because no saved connection exists for the SSID.
    PasswordRequired(String),
    /// A connection attempt completed.
    ConnectFinished(Result<(), String>),
    /// A backend error occurred.
    Error(String),
}

/// NetworkManager Wi-Fi controller.
pub struct WifiManager;

impl WifiManager {
    /// Scans nearby access points through NetworkManager.
    pub async fn scan() -> Result<Vec<WifiNetwork>> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let device = first_wifi_device(&connection).await?;
        let wireless = proxy(&connection, device.as_str(), NM_WIRELESS_IFACE).await?;
        let previous_last_scan = wireless.get_property("LastScan").await.unwrap_or(-1i64);
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let _: () = wireless
            .call("RequestScan", &(options))
            .await
            .context("failed to request Wi-Fi scan")?;

        wait_for_scan_completion(&wireless, previous_last_scan).await;

        let access_points: Vec<OwnedObjectPath> = wireless
            .call("GetAccessPoints", &())
            .await
            .context("failed to list Wi-Fi access points")?;
        let active = Self::active_ssid().await?;
        let mut networks = Vec::new();

        for access_point in access_points {
            if let Ok(Some(network)) =
                access_point_to_network(&connection, &access_point, &active).await
            {
                networks.push(network);
            }
        }

        networks.sort_by(|left, right| {
            right
                .strength
                .cmp(&left.strength)
                .then_with(|| left.ssid.to_lowercase().cmp(&right.ssid.to_lowercase()))
        });
        networks.dedup_by(|left, right| left.ssid == right.ssid);

        Ok(networks)
    }

    /// Connects to a saved or newly created Wi-Fi connection.
    pub async fn connect(ssid: &str, password: Option<&str>) -> Result<()> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let device = first_wifi_device(&connection).await?;
        let manager = proxy(&connection, NM_PATH, NM_IFACE).await?;

        if let Some(saved) = saved_connection_for_ssid(&connection, ssid).await? {
            let root = ObjectPath::try_from("/")?;
            let _: OwnedObjectPath = manager
                .call("ActivateConnection", &(saved, device, root))
                .await
                .with_context(|| format!("failed to activate saved Wi-Fi connection {ssid}"))?;
            return Ok(());
        }

        let access_point = access_point_for_ssid(&connection, &device, ssid)
            .await?
            .ok_or_else(|| anyhow!("could not find access point for SSID {ssid}"))?;
        let settings = new_connection_settings(ssid, password);
        let _: (OwnedObjectPath, OwnedObjectPath) = manager
            .call(
                "AddAndActivateConnection",
                &(settings, device, access_point),
            )
            .await
            .with_context(|| format!("failed to add Wi-Fi connection {ssid}"))?;

        Ok(())
    }

    /// Returns whether NetworkManager already has a saved connection for an SSID.
    pub async fn has_saved_connection(ssid: &str) -> Result<bool> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        saved_connection_for_ssid(&connection, ssid)
            .await
            .map(|connection| connection.is_some())
    }

    /// Returns the currently active Wi-Fi SSID, if any.
    pub async fn active_ssid() -> Result<Option<String>> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let device = first_wifi_device(&connection).await?;
        let wireless = proxy(&connection, device.as_str(), NM_WIRELESS_IFACE).await?;
        let active_ap: OwnedObjectPath = wireless
            .get_property("ActiveAccessPoint")
            .await
            .context("failed to read active Wi-Fi access point")?;

        if active_ap.as_str() == "/" {
            return Ok(None);
        }

        let ap = proxy(&connection, active_ap.as_str(), NM_AP_IFACE).await?;
        let ssid: Vec<u8> = ap
            .get_property("Ssid")
            .await
            .context("failed to read active Wi-Fi SSID")?;

        Ok(Some(ssid_to_string(&ssid)))
    }

    /// Starts a Tokio-backed Wi-Fi worker and sends events to the provided GTK-side sender.
    pub fn spawn_worker(sender: Sender<WifiEvent>) -> Result<WifiWorker> {
        WifiWorker::new(sender)
    }
}

/// Handle for the background Wi-Fi worker.
#[derive(Clone)]
pub struct WifiWorker {
    sender: tokio::sync::mpsc::UnboundedSender<WifiCommand>,
}

impl WifiWorker {
    fn new(event_sender: Sender<WifiEvent>) -> Result<Self> {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<WifiCommand>();
        thread::Builder::new()
            .name("gameease-wifi".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_sender.send(WifiEvent::Error(format!(
                            "failed to create Wi-Fi runtime: {error:#}"
                        )));
                        return;
                    }
                };

                runtime.block_on(async move {
                    while let Some(command) = receiver.recv().await {
                        let event_sender = event_sender.clone();
                        tokio::spawn(async move {
                            match command {
                                WifiCommand::Scan => {
                                    let _ = event_sender.send(WifiEvent::ScanStarted);
                                    match WifiManager::scan().await {
                                        Ok(networks) => {
                                            let _ = event_sender
                                                .send(WifiEvent::ScanFinished(networks));
                                        }
                                        Err(error) => {
                                            let _ = event_sender
                                                .send(WifiEvent::Error(error.to_string()));
                                        }
                                    }
                                }
                                WifiCommand::Connect { ssid, password } => {
                                    let _ =
                                        event_sender.send(WifiEvent::ConnectStarted(ssid.clone()));
                                    let result = WifiManager::connect(&ssid, password.as_deref())
                                        .await
                                        .map_err(|error| error.to_string());
                                    send_connect_result_and_refresh(&event_sender, result).await;
                                }
                                WifiCommand::ConnectOrRequestPassword { ssid } => {
                                    match WifiManager::has_saved_connection(&ssid).await {
                                        Ok(true) => {
                                            let _ = event_sender
                                                .send(WifiEvent::ConnectStarted(ssid.clone()));
                                            let result = WifiManager::connect(&ssid, None)
                                                .await
                                                .map_err(|error| error.to_string());
                                            send_connect_result_and_refresh(&event_sender, result)
                                                .await;
                                        }
                                        Ok(false) => {
                                            let _ = event_sender
                                                .send(WifiEvent::PasswordRequired(ssid));
                                        }
                                        Err(error) => {
                                            let _ = event_sender
                                                .send(WifiEvent::Error(error.to_string()));
                                        }
                                    }
                                }
                            }
                        });
                    }
                });
            })
            .context("failed to spawn Wi-Fi worker")?;

        Ok(Self { sender })
    }

    /// Requests a fresh network scan.
    pub fn scan(&self) {
        let _ = self.sender.send(WifiCommand::Scan);
    }

    /// Requests a Wi-Fi connection attempt.
    pub fn connect(&self, ssid: String, password: Option<String>) {
        let _ = self.sender.send(WifiCommand::Connect { ssid, password });
    }

    /// Activates a saved Wi-Fi connection or asks GTK to collect a password.
    pub fn connect_or_request_password(&self, ssid: String) {
        let _ = self
            .sender
            .send(WifiCommand::ConnectOrRequestPassword { ssid });
    }
}

enum WifiCommand {
    Scan,
    Connect {
        ssid: String,
        password: Option<String>,
    },
    ConnectOrRequestPassword {
        ssid: String,
    },
}

async fn send_connect_result_and_refresh(
    event_sender: &Sender<WifiEvent>,
    result: Result<(), String>,
) {
    let connect_succeeded = result.is_ok();
    let _ = event_sender.send(WifiEvent::ConnectFinished(result));

    if connect_succeeded {
        match WifiManager::scan().await {
            Ok(networks) => {
                let _ = event_sender.send(WifiEvent::ScanFinished(networks));
            }
            Err(error) => {
                let _ = event_sender.send(WifiEvent::Error(error.to_string()));
            }
        }
    }
}

async fn proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> Result<Proxy<'a>> {
    Proxy::new(connection, NM_DEST, path, interface)
        .await
        .with_context(|| format!("failed to create D-Bus proxy for {interface} at {path}"))
}

async fn first_wifi_device(connection: &Connection) -> Result<OwnedObjectPath> {
    let manager = proxy(connection, NM_PATH, NM_IFACE).await?;
    let devices: Vec<OwnedObjectPath> = manager
        .call("GetDevices", &())
        .await
        .context("failed to list NetworkManager devices")?;

    for device in devices {
        let device_proxy = proxy(connection, device.as_str(), NM_DEVICE_IFACE).await?;
        let device_type: u32 = device_proxy
            .get_property("DeviceType")
            .await
            .context("failed to read NetworkManager device type")?;
        if device_type == NM_DEVICE_TYPE_WIFI {
            return Ok(device);
        }
    }

    Err(anyhow!("no NetworkManager Wi-Fi device found"))
}

async fn access_point_to_network(
    connection: &Connection,
    access_point: &OwnedObjectPath,
    active_ssid: &Option<String>,
) -> Result<Option<WifiNetwork>> {
    let ap = proxy(connection, access_point.as_str(), NM_AP_IFACE).await?;
    let ssid_bytes: Vec<u8> = ap
        .get_property("Ssid")
        .await
        .context("failed to read SSID")?;
    let ssid = ssid_to_string(&ssid_bytes);
    if ssid.is_empty() {
        return Ok(None);
    }

    let strength: u8 = ap
        .get_property("Strength")
        .await
        .context("failed to read Wi-Fi signal strength")?;
    let flags: u32 = ap.get_property("Flags").await.unwrap_or(0);
    let wpa_flags: u32 = ap.get_property("WpaFlags").await.unwrap_or(0);
    let rsn_flags: u32 = ap.get_property("RsnFlags").await.unwrap_or(0);
    let secured = (flags & NM_802_11_AP_FLAGS_PRIVACY) != 0 || wpa_flags != 0 || rsn_flags != 0;
    let connected = active_ssid.as_ref().is_some_and(|active| active == &ssid);

    Ok(Some(WifiNetwork {
        ssid,
        strength,
        secured,
        connected,
    }))
}

async fn wait_for_scan_completion(wireless: &Proxy<'_>, previous_last_scan: i64) {
    let deadline = tokio::time::Instant::now() + SCAN_TIMEOUT;

    loop {
        tokio::time::sleep(SCAN_POLL_INTERVAL).await;

        match wireless.get_property::<i64>("LastScan").await {
            Ok(last_scan) if last_scan > previous_last_scan && last_scan > 0 => break,
            _ if tokio::time::Instant::now() >= deadline => break,
            _ => {}
        }
    }

    tokio::time::sleep(Duration::from_millis(250)).await;
}

async fn access_point_for_ssid(
    connection: &Connection,
    device: &OwnedObjectPath,
    ssid: &str,
) -> Result<Option<OwnedObjectPath>> {
    let wireless = proxy(connection, device.as_str(), NM_WIRELESS_IFACE).await?;
    let access_points: Vec<OwnedObjectPath> = wireless
        .call("GetAccessPoints", &())
        .await
        .context("failed to list Wi-Fi access points")?;

    for access_point in access_points {
        let ap = proxy(connection, access_point.as_str(), NM_AP_IFACE).await?;
        let ssid_bytes: Vec<u8> = ap
            .get_property("Ssid")
            .await
            .context("failed to read SSID")?;
        if ssid_to_string(&ssid_bytes) == ssid {
            return Ok(Some(access_point));
        }
    }

    Ok(None)
}

async fn saved_connection_for_ssid(
    connection: &Connection,
    ssid: &str,
) -> Result<Option<OwnedObjectPath>> {
    let settings = proxy(connection, NM_SETTINGS_PATH, NM_SETTINGS_IFACE).await?;
    let connections: Vec<OwnedObjectPath> = settings
        .call("ListConnections", &())
        .await
        .context("failed to list saved NetworkManager connections")?;

    for connection_path in connections {
        let connection_proxy = proxy(
            connection,
            connection_path.as_str(),
            NM_SETTINGS_CONNECTION_IFACE,
        )
        .await?;
        let settings: HashMap<String, HashMap<String, OwnedValue>> =
            match connection_proxy.call("GetSettings", &()).await {
                Ok(settings) => settings,
                Err(error) => {
                    eprintln!(
                        "Skipping unreadable NetworkManager connection {}: {error}",
                        connection_path.as_str()
                    );
                    continue;
                }
            };

        if settings_ssid(&settings).as_deref() == Some(ssid) {
            return Ok(Some(connection_path));
        }
    }

    Ok(None)
}

fn settings_ssid(settings: &HashMap<String, HashMap<String, OwnedValue>>) -> Option<String> {
    let wireless = settings.get("802-11-wireless")?;
    let ssid = Vec::<u8>::try_from(wireless.get("ssid")?.try_clone().ok()?).ok()?;
    Some(ssid_to_string(&ssid))
}

fn new_connection_settings(
    ssid: &str,
    password: Option<&str>,
) -> HashMap<String, HashMap<String, Value<'static>>> {
    let mut settings = HashMap::new();

    let mut connection = HashMap::new();
    connection.insert("id".to_string(), Value::from(ssid.to_string()));
    connection.insert("type".to_string(), Value::from("802-11-wireless"));
    settings.insert("connection".to_string(), connection);

    let mut wireless = HashMap::new();
    wireless.insert("ssid".to_string(), Value::from(ssid.as_bytes().to_vec()));
    wireless.insert("mode".to_string(), Value::from("infrastructure"));

    let mut ipv4 = HashMap::new();
    ipv4.insert("method".to_string(), Value::from("auto"));
    settings.insert("ipv4".to_string(), ipv4);

    let mut ipv6 = HashMap::new();
    ipv6.insert("method".to_string(), Value::from("auto"));
    settings.insert("ipv6".to_string(), ipv6);

    if let Some(password) = password.filter(|password| !password.is_empty()) {
        wireless.insert(
            "security".to_string(),
            Value::from("802-11-wireless-security"),
        );

        let mut security = HashMap::new();
        security.insert("key-mgmt".to_string(), Value::from("wpa-psk"));
        security.insert("psk".to_string(), Value::from(password.to_string()));
        settings.insert("802-11-wireless-security".to_string(), security);
    }

    settings.insert("802-11-wireless".to_string(), wireless);

    settings
}

fn ssid_to_string(ssid: &[u8]) -> String {
    String::from_utf8_lossy(ssid)
        .trim_matches(char::from(0))
        .to_string()
}
