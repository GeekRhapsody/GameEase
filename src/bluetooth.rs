use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use zbus::export::futures_util::StreamExt;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

const BLUEZ_DEST: &str = "org.bluez";
const BLUEZ_ROOT: &str = "/";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const SCAN_DURATION: Duration = Duration::from_secs(8);

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// Bluetooth device details from BlueZ.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BluetoothDevice {
    /// BlueZ object path used as a stable ID.
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// Device MAC address.
    pub address: String,
    /// Whether the device is paired.
    pub paired: bool,
    /// Whether the device is currently connected.
    pub connected: bool,
    /// Whether the device appears to be a game controller.
    pub gamepad: bool,
}

/// Messages emitted by the Bluetooth worker for the GTK side menu.
#[derive(Debug)]
pub enum BluetoothEvent {
    /// Device discovery started.
    ScanStarted,
    /// Device list changed.
    DevicesUpdated(Vec<BluetoothDevice>),
    /// An operation started for the named device.
    OperationStarted(String),
    /// An operation completed.
    OperationFinished(Result<(), String>),
    /// A backend error occurred.
    Error(String),
}

/// BlueZ Bluetooth controller.
pub struct BluetoothManager;

impl BluetoothManager {
    /// Lists paired and discovered devices through BlueZ ObjectManager.
    pub async fn list_devices() -> Result<Vec<BluetoothDevice>> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let objects = managed_objects(&connection).await?;
        Ok(devices_from_objects(&objects))
    }

    /// Starts discovery and reports newly discovered devices while scanning.
    pub async fn scan(event_sender: Sender<BluetoothEvent>) -> Result<()> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let adapter_path = first_adapter(&connection).await?;
        let manager = proxy(&connection, BLUEZ_ROOT, OBJECT_MANAGER_IFACE).await?;
        let mut interfaces_added = manager
            .receive_signal("InterfacesAdded")
            .await
            .context("failed to subscribe to BlueZ InterfacesAdded")?;
        let adapter = proxy(&connection, adapter_path.as_str(), ADAPTER_IFACE).await?;

        let _: () = adapter
            .call("StartDiscovery", &())
            .await
            .context("failed to start Bluetooth discovery")?;

        let _ = event_sender.send(BluetoothEvent::DevicesUpdated(Self::list_devices().await?));
        let deadline = tokio::time::Instant::now() + SCAN_DURATION;

        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(
                remaining.min(Duration::from_millis(750)),
                interfaces_added.next(),
            )
            .await
            {
                Ok(Some(message)) if interfaces_added_has_device(&message) => {
                    let _ = event_sender
                        .send(BluetoothEvent::DevicesUpdated(Self::list_devices().await?));
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    let _ = event_sender
                        .send(BluetoothEvent::DevicesUpdated(Self::list_devices().await?));
                }
            }
        }

        let _: Result<(), zbus::Error> = adapter.call("StopDiscovery", &()).await;
        Ok(())
    }

    /// Connects an already paired device.
    pub async fn connect(device_path: &str) -> Result<()> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let device = proxy(&connection, device_path, DEVICE_IFACE).await?;
        let _: () = device
            .call("Connect", &())
            .await
            .context("failed to connect Bluetooth device")?;
        Ok(())
    }

    /// Pairs a device and then connects it.
    pub async fn pair_and_connect(device_path: &str) -> Result<()> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let device = proxy(&connection, device_path, DEVICE_IFACE).await?;
        let _: () = device
            .call("Pair", &())
            .await
            .context("failed to pair Bluetooth device")?;
        let _: () = device
            .call("Connect", &())
            .await
            .context("failed to connect Bluetooth device")?;
        Ok(())
    }

    /// Disconnects a connected device.
    pub async fn disconnect(device_path: &str) -> Result<()> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let device = proxy(&connection, device_path, DEVICE_IFACE).await?;
        let _: () = device
            .call("Disconnect", &())
            .await
            .context("failed to disconnect Bluetooth device")?;
        Ok(())
    }

    /// Removes a paired device from the first local adapter.
    pub async fn remove(device_path: &str) -> Result<()> {
        let connection = Connection::system()
            .await
            .context("failed to connect to system D-Bus")?;
        let adapter_path = first_adapter(&connection).await?;
        let adapter = proxy(&connection, adapter_path.as_str(), ADAPTER_IFACE).await?;
        let device = ObjectPath::try_from(device_path)
            .with_context(|| format!("invalid Bluetooth device path {device_path}"))?;
        let _: () = adapter
            .call("RemoveDevice", &(device))
            .await
            .context("failed to remove Bluetooth device")?;
        Ok(())
    }

    /// Starts a Tokio-backed Bluetooth worker and sends events to the provided GTK-side sender.
    pub fn spawn_worker(sender: Sender<BluetoothEvent>) -> Result<BluetoothWorker> {
        BluetoothWorker::new(sender)
    }
}

/// Handle for the background Bluetooth worker.
#[derive(Clone)]
pub struct BluetoothWorker {
    sender: tokio::sync::mpsc::UnboundedSender<BluetoothCommand>,
}

impl BluetoothWorker {
    fn new(event_sender: Sender<BluetoothEvent>) -> Result<Self> {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<BluetoothCommand>();
        thread::Builder::new()
            .name("gameease-bluetooth".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = event_sender.send(BluetoothEvent::Error(format!(
                            "failed to create Bluetooth runtime: {error:#}"
                        )));
                        return;
                    }
                };

                runtime.block_on(async move {
                    while let Some(command) = receiver.recv().await {
                        let event_sender = event_sender.clone();
                        tokio::spawn(async move {
                            match command {
                                BluetoothCommand::Refresh => {
                                    match BluetoothManager::list_devices().await {
                                        Ok(devices) => {
                                            let _ = event_sender
                                                .send(BluetoothEvent::DevicesUpdated(devices));
                                        }
                                        Err(error) => {
                                            let _ = event_sender
                                                .send(BluetoothEvent::Error(error.to_string()));
                                        }
                                    }
                                }
                                BluetoothCommand::Scan => {
                                    let _ = event_sender.send(BluetoothEvent::ScanStarted);
                                    if let Err(error) =
                                        BluetoothManager::scan(event_sender.clone()).await
                                    {
                                        let _ = event_sender
                                            .send(BluetoothEvent::Error(error.to_string()));
                                    }
                                }
                                BluetoothCommand::Connect { path, name } => {
                                    run_operation(event_sender, name, async move {
                                        BluetoothManager::connect(&path).await
                                    })
                                    .await;
                                }
                                BluetoothCommand::PairConnect { path, name } => {
                                    run_operation(event_sender, name, async move {
                                        BluetoothManager::pair_and_connect(&path).await
                                    })
                                    .await;
                                }
                                BluetoothCommand::Disconnect { path, name } => {
                                    run_operation(event_sender, name, async move {
                                        BluetoothManager::disconnect(&path).await
                                    })
                                    .await;
                                }
                                BluetoothCommand::Remove { path, name } => {
                                    run_operation(event_sender, name, async move {
                                        BluetoothManager::remove(&path).await
                                    })
                                    .await;
                                }
                            }
                        });
                    }
                });
            })
            .context("failed to spawn Bluetooth worker")?;

        Ok(Self { sender })
    }

    /// Requests a device list refresh.
    pub fn refresh(&self) {
        let _ = self.sender.send(BluetoothCommand::Refresh);
    }

    /// Starts discovery.
    pub fn scan(&self) {
        let _ = self.sender.send(BluetoothCommand::Scan);
    }

    /// Connects a paired device.
    pub fn connect(&self, path: String, name: String) {
        let _ = self.sender.send(BluetoothCommand::Connect { path, name });
    }

    /// Pairs and connects an unpaired device.
    pub fn pair_and_connect(&self, path: String, name: String) {
        let _ = self
            .sender
            .send(BluetoothCommand::PairConnect { path, name });
    }

    /// Disconnects a connected device.
    pub fn disconnect(&self, path: String, name: String) {
        let _ = self
            .sender
            .send(BluetoothCommand::Disconnect { path, name });
    }

    /// Removes a paired device.
    pub fn remove(&self, path: String, name: String) {
        let _ = self.sender.send(BluetoothCommand::Remove { path, name });
    }
}

enum BluetoothCommand {
    Refresh,
    Scan,
    Connect { path: String, name: String },
    PairConnect { path: String, name: String },
    Disconnect { path: String, name: String },
    Remove { path: String, name: String },
}

async fn run_operation(
    event_sender: Sender<BluetoothEvent>,
    name: String,
    operation: impl std::future::Future<Output = Result<()>>,
) {
    let _ = event_sender.send(BluetoothEvent::OperationStarted(name));
    let result = operation.await.map_err(|error| error.to_string());
    let succeeded = result.is_ok();
    let _ = event_sender.send(BluetoothEvent::OperationFinished(result));
    if succeeded {
        match BluetoothManager::list_devices().await {
            Ok(devices) => {
                let _ = event_sender.send(BluetoothEvent::DevicesUpdated(devices));
            }
            Err(error) => {
                let _ = event_sender.send(BluetoothEvent::Error(error.to_string()));
            }
        }
    }
}

async fn proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> Result<Proxy<'a>> {
    Proxy::new(connection, BLUEZ_DEST, path, interface)
        .await
        .with_context(|| format!("failed to create D-Bus proxy for {interface} at {path}"))
}

async fn managed_objects(connection: &Connection) -> Result<ManagedObjects> {
    let manager = proxy(connection, BLUEZ_ROOT, OBJECT_MANAGER_IFACE).await?;
    manager
        .call("GetManagedObjects", &())
        .await
        .context("failed to read BlueZ managed objects")
}

async fn first_adapter(connection: &Connection) -> Result<OwnedObjectPath> {
    let objects = managed_objects(connection).await?;
    objects
        .iter()
        .find_map(|(path, interfaces)| {
            interfaces
                .contains_key(ADAPTER_IFACE)
                .then_some(path.clone())
        })
        .ok_or_else(|| anyhow!("no BlueZ Bluetooth adapter found"))
}

fn devices_from_objects(objects: &ManagedObjects) -> Vec<BluetoothDevice> {
    let mut devices = Vec::new();

    for (path, interfaces) in objects {
        let Some(props) = interfaces.get(DEVICE_IFACE) else {
            continue;
        };
        let address = string_prop(props, "Address").unwrap_or_default();
        let name = string_prop(props, "Alias")
            .or_else(|| string_prop(props, "Name"))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| address.clone());
        let paired = bool_prop(props, "Paired").unwrap_or(false);
        let connected = bool_prop(props, "Connected").unwrap_or(false);
        let icon = string_prop(props, "Icon").unwrap_or_default();
        let uuids = string_vec_prop(props, "UUIDs").unwrap_or_default();
        let gamepad = icon.contains("gaming")
            || icon.contains("input")
            || name.to_lowercase().contains("controller")
            || name.to_lowercase().contains("gamepad")
            || uuids.iter().any(|uuid| uuid.starts_with("00001124-"));

        devices.push(BluetoothDevice {
            id: path.to_string(),
            name,
            address,
            paired,
            connected,
            gamepad,
        });
    }

    devices.sort_by(|left, right| {
        right
            .connected
            .cmp(&left.connected)
            .then_with(|| right.paired.cmp(&left.paired))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    devices
}

fn interfaces_added_has_device(message: &zbus::Message) -> bool {
    let Ok((_, interfaces)) = message.body().deserialize::<(
        OwnedObjectPath,
        HashMap<String, HashMap<String, OwnedValue>>,
    )>() else {
        return false;
    };
    interfaces.contains_key(DEVICE_IFACE)
}

fn string_prop(props: &HashMap<String, OwnedValue>, name: &str) -> Option<String> {
    String::try_from(props.get(name)?.try_clone().ok()?).ok()
}

fn string_vec_prop(props: &HashMap<String, OwnedValue>, name: &str) -> Option<Vec<String>> {
    Vec::<String>::try_from(props.get(name)?.try_clone().ok()?).ok()
}

fn bool_prop(props: &HashMap<String, OwnedValue>, name: &str) -> Option<bool> {
    bool::try_from(props.get(name)?.try_clone().ok()?).ok()
}
