use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tokio::sync::mpsc;
use zbus::{interface, Connection, Proxy};

/// A visible application or game window exposed by the compositor.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TaskEntry {
    /// Backend-specific window identifier.
    pub id: String,
    /// Display title shown in the task switcher.
    pub title: String,
    /// Application identifier or window class.
    pub app_id: String,
    /// Process ID associated with the window, when the compositor exposes it.
    pub pid: Option<u32>,
    /// Whether the compositor currently marks this window as focused.
    pub active: bool,
}

/// Compositor-backed task switcher actions.
pub struct TaskManager;

impl TaskManager {
    /// Lists open application windows from the active compositor.
    pub fn list() -> Result<Vec<TaskEntry>> {
        let mut errors = Vec::new();

        for backend in backend_candidates() {
            let result = match backend {
                Backend::Hyprland => list_hyprland_tasks(),
                Backend::Sway => list_sway_tasks(),
                Backend::KWin => list_kwin_tasks(),
            };

            match result {
                Ok(tasks) => return Ok(tasks),
                Err(error) => errors.push(format!("{}: {error:#}", backend.name())),
            }
        }

        Err(anyhow!(
            "no supported compositor task backend available ({})",
            errors.join("; ")
        ))
    }

    /// Makes a task's window active.
    pub fn focus(task_id: &str) -> Result<()> {
        let (backend, id) = split_task_id(task_id)?;

        match backend {
            Backend::Hyprland => run_status_for_backend(
                backend,
                "hyprctl",
                &["dispatch", "focuswindow", &format!("address:{id}")],
            ),
            Backend::Sway => {
                run_status_for_backend(backend, "swaymsg", &[&format!("[con_id={id}]"), "focus"])
            }
            Backend::KWin => run_kwin_script(&kwin_focus_script(id)).map(|_| ()),
        }
    }

    /// Sends SIGTERM to the process behind a task.
    pub fn terminate(task: &TaskEntry) -> Result<()> {
        let pid = task.pid.ok_or_else(|| {
            anyhow!(
                "the compositor did not expose a process ID for {}",
                task.title
            )
        })?;
        run_status("kill", &["-TERM", &pid.to_string()])
            .with_context(|| format!("failed to terminate {}", task.title))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Backend {
    Hyprland,
    Sway,
    KWin,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Hyprland => "Hyprland",
            Backend::Sway => "Sway",
            Backend::KWin => "KWin",
        }
    }
}

#[derive(Deserialize)]
struct HyprClient {
    address: String,
    title: String,
    #[serde(rename = "class")]
    class_name: String,
    pid: i64,
    #[serde(rename = "focusHistoryID")]
    focus_history_id: i64,
    #[serde(default = "default_true")]
    mapped: bool,
}

#[derive(Deserialize)]
struct SwayNode {
    id: i64,
    name: Option<String>,
    app_id: Option<String>,
    pid: Option<i64>,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    nodes: Vec<SwayNode>,
    #[serde(default)]
    floating_nodes: Vec<SwayNode>,
    window_properties: Option<SwayWindowProperties>,
}

#[derive(Deserialize)]
struct SwayWindowProperties {
    class: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct KWinTask {
    id: String,
    title: String,
    app_id: String,
    pid: Option<i64>,
    active: bool,
}

fn backend_candidates() -> Vec<Backend> {
    let mut candidates = Vec::new();

    if is_kde_session() {
        candidates.push(Backend::KWin);
    }
    if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        candidates.push(Backend::Hyprland);
    }
    if env::var_os("SWAYSOCK").is_some() {
        candidates.push(Backend::Sway);
    }
    if !candidates.contains(&Backend::Hyprland) && discover_hyprland_signature().is_some() {
        candidates.push(Backend::Hyprland);
    }
    if !candidates.contains(&Backend::Sway) && discover_sway_socket().is_some() {
        candidates.push(Backend::Sway);
    }
    if !candidates.contains(&Backend::KWin) {
        candidates.push(Backend::KWin);
    }
    if !candidates.contains(&Backend::Hyprland) {
        candidates.push(Backend::Hyprland);
    }
    if !candidates.contains(&Backend::Sway) {
        candidates.push(Backend::Sway);
    }

    candidates
}

fn default_true() -> bool {
    true
}

fn list_hyprland_tasks() -> Result<Vec<TaskEntry>> {
    let active_output =
        run_output_for_backend(Backend::Hyprland, "hyprctl", &["activewindow", "-j"])
            .unwrap_or_default();
    let active_address = serde_json::from_str::<serde_json::Value>(&active_output)
        .ok()
        .and_then(|value| {
            value
                .get("address")
                .and_then(|address| address.as_str())
                .map(str::to_string)
        });
    let output = run_output_for_backend(Backend::Hyprland, "hyprctl", &["clients", "-j"])?;
    let clients: Vec<HyprClient> =
        serde_json::from_str(&output).context("failed to parse Hyprland clients")?;
    let mut tasks = clients
        .into_iter()
        .filter(|client| client.mapped)
        .filter(|client| !client.title.is_empty() || !client.class_name.is_empty())
        .map(|client| {
            let title = if client.title.is_empty() {
                client.class_name.clone()
            } else {
                client.title.clone()
            };
            let active = active_address
                .as_ref()
                .is_some_and(|address| address == &client.address)
                || client.focus_history_id == 0;

            TaskEntry {
                id: format!("hypr:{}", client.address),
                title,
                app_id: client.class_name,
                pid: u32::try_from(client.pid).ok(),
                active,
            }
        })
        .collect::<Vec<_>>();

    sort_tasks(&mut tasks);
    Ok(tasks)
}

fn list_sway_tasks() -> Result<Vec<TaskEntry>> {
    let output = run_output_for_backend(Backend::Sway, "swaymsg", &["-t", "get_tree"])?;
    let tree: SwayNode = serde_json::from_str(&output).context("failed to parse Sway tree")?;
    let mut tasks = Vec::new();
    collect_sway_tasks(&tree, &mut tasks);
    sort_tasks(&mut tasks);
    Ok(tasks)
}

fn list_kwin_tasks() -> Result<Vec<TaskEntry>> {
    let output = run_kwin_script(KWIN_LIST_SCRIPT)?;
    let kwin_tasks: Vec<KWinTask> =
        serde_json::from_str(&output).context("failed to parse KWin window list")?;
    let mut tasks = kwin_tasks
        .into_iter()
        .filter(|task| !task.title.is_empty() || !task.app_id.is_empty())
        .map(|task| TaskEntry {
            id: format!("kwin:{}", task.id),
            title: if task.title.is_empty() {
                task.app_id.clone()
            } else {
                task.title
            },
            app_id: task.app_id,
            pid: task.pid.and_then(|pid| u32::try_from(pid).ok()),
            active: task.active,
        })
        .collect::<Vec<_>>();

    sort_tasks(&mut tasks);
    Ok(tasks)
}

fn collect_sway_tasks(node: &SwayNode, tasks: &mut Vec<TaskEntry>) {
    let app_id = node
        .app_id
        .clone()
        .or_else(|| {
            node.window_properties
                .as_ref()
                .and_then(|properties| properties.class.clone())
        })
        .unwrap_or_default();
    let title = node
        .window_properties
        .as_ref()
        .and_then(|properties| properties.title.clone())
        .or_else(|| node.name.clone())
        .unwrap_or_else(|| app_id.clone());

    if node.pid.is_some() && (!title.is_empty() || !app_id.is_empty()) {
        tasks.push(TaskEntry {
            id: format!("sway:{}", node.id),
            title: if title.is_empty() {
                app_id.clone()
            } else {
                title
            },
            app_id,
            pid: node.pid.and_then(|pid| u32::try_from(pid).ok()),
            active: node.focused,
        });
    }

    for child in node.nodes.iter().chain(node.floating_nodes.iter()) {
        collect_sway_tasks(child, tasks);
    }
}

fn sort_tasks(tasks: &mut [TaskEntry]) {
    tasks.sort_by(|left, right| {
        right
            .active
            .cmp(&left.active)
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
}

fn split_task_id(task_id: &str) -> Result<(Backend, &str)> {
    if let Some(id) = task_id.strip_prefix("hypr:") {
        Ok((Backend::Hyprland, id))
    } else if let Some(id) = task_id.strip_prefix("sway:") {
        Ok((Backend::Sway, id))
    } else if let Some(id) = task_id.strip_prefix("kwin:") {
        Ok((Backend::KWin, id))
    } else {
        Err(anyhow!("unsupported task id {task_id}"))
    }
}

fn run_output_for_backend(backend: Backend, program: &str, args: &[&str]) -> Result<String> {
    let output = command_for_backend(backend, program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;

    if !output.status.success() {
        return Err(command_failure(program, &output));
    }

    String::from_utf8(output.stdout).with_context(|| format!("{program} returned invalid UTF-8"))
}

fn run_status_for_backend(backend: Backend, program: &str, args: &[&str]) -> Result<()> {
    let output = command_for_backend(backend, program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(command_failure(program, &output))
    }
}

fn run_status(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(command_failure(program, &output))
    }
}

fn command_failure(program: &str, output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };

    anyhow!("{program} failed: {details}")
}

fn command_for_backend(backend: Backend, program: &str) -> Command {
    let mut command = Command::new(program);

    for (key, value) in backend_env(backend) {
        command.env(key, value);
    }

    command
}

fn backend_env(backend: Backend) -> Vec<(&'static str, String)> {
    match backend {
        Backend::Hyprland if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() => {
            discover_hyprland_signature()
                .map(|signature| vec![("HYPRLAND_INSTANCE_SIGNATURE", signature)])
                .unwrap_or_default()
        }
        Backend::Sway if env::var_os("SWAYSOCK").is_none() => discover_sway_socket()
            .map(|socket| vec![("SWAYSOCK", socket)])
            .unwrap_or_default(),
        Backend::KWin => Vec::new(),
        _ => Vec::new(),
    }
}

const KWIN_BRIDGE_PATH: &str = "/org/gameease/KWinBridge";
const KWIN_BRIDGE_INTERFACE: &str = "org.gameease.KWinBridge";
const KWIN_SCRIPT_TIMEOUT: Duration = Duration::from_secs(4);

const KWIN_LIST_SCRIPT: &str = r#"
const active = workspace.activeWindow;
const windows = ge_windows();
const tasks = [];

for (let i = 0; i < windows.length; i++) {
    const w = windows[i];
    if (w.managed === false || w.deleted || w.desktopWindow || w.dock || w.skipTaskbar || w.skipSwitcher) {
        continue;
    }

    const title = String(w.caption || "");
    const appId = String(w.desktopFileName || w.resourceClass || w.resourceName || "");
    if (title.length === 0 && appId.length === 0) {
        continue;
    }

    tasks.push({
        id: String(w.internalId),
        title: title,
        app_id: appId,
        pid: Number.isFinite(w.pid) && w.pid > 0 ? w.pid : null,
        active: Boolean(active && String(active.internalId) === String(w.internalId))
    });
}

ge_result(JSON.stringify(tasks));
"#;

enum KWinScriptMessage {
    Result(String),
    Error(String),
}

struct KWinBridge {
    sender: mpsc::UnboundedSender<KWinScriptMessage>,
}

#[interface(name = "org.gameease.KWinBridge")]
impl KWinBridge {
    #[zbus(name = "Result")]
    fn result(&self, payload: &str) -> zbus::fdo::Result<()> {
        self.send(KWinScriptMessage::Result(payload.to_string()))
    }

    #[zbus(name = "Error")]
    fn error(&self, payload: &str) -> zbus::fdo::Result<()> {
        self.send(KWinScriptMessage::Error(payload.to_string()))
    }
}

impl KWinBridge {
    fn send(&self, message: KWinScriptMessage) -> zbus::fdo::Result<()> {
        self.sender
            .send(message)
            .map_err(|_| zbus::fdo::Error::Failed("KWin bridge receiver was closed".into()))
    }
}

fn run_kwin_script(body: &str) -> Result<String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start a Tokio runtime for KWin task switching")?;

    runtime.block_on(run_kwin_script_async(body))
}

async fn run_kwin_script_async(body: &str) -> Result<String> {
    let connection = Connection::session()
        .await
        .context("failed to connect to the session D-Bus")?;
    let service = connection
        .unique_name()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("session D-Bus did not assign GameEase a unique name"))?;
    let (sender, mut receiver) = mpsc::unbounded_channel();
    connection
        .object_server()
        .at(KWIN_BRIDGE_PATH, KWinBridge { sender })
        .await
        .context("failed to expose the GameEase KWin bridge on D-Bus")?;

    let script = kwin_script(&service, body)?;
    let script_path = write_kwin_script(&script)?;
    let script_name = format!(
        "gameease-kwin-{}-{}",
        process::id(),
        unix_time_nanos().unwrap_or_default()
    );
    let script_path_string = script_path.to_string_lossy().to_string();

    let scripting_proxy = Proxy::new(
        &connection,
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting",
    )
    .await
    .context("failed to create KWin scripting D-Bus proxy")?;
    let script_id: i32 = scripting_proxy
        .call(
            "loadScript",
            &(script_path_string.as_str(), script_name.as_str()),
        )
        .await
        .context("failed to load KWin task script")?;
    if script_id < 0 {
        let _ = fs::remove_file(&script_path);
        return Err(anyhow!("KWin refused to load the task script"));
    }

    let script_object_path = format!("/Scripting/Script{script_id}");
    let script_proxy = Proxy::new(
        &connection,
        "org.kde.KWin",
        script_object_path.as_str(),
        "org.kde.kwin.Script",
    )
    .await
    .context("failed to create KWin task script D-Bus proxy")?;

    let result = async {
        let _: () = script_proxy
            .call("run", &())
            .await
            .context("failed to run KWin task script")?;
        match tokio::time::timeout(KWIN_SCRIPT_TIMEOUT, receiver.recv())
            .await
            .context("timed out waiting for KWin task script output")?
            .ok_or_else(|| anyhow!("KWin task script output channel closed"))?
        {
            KWinScriptMessage::Result(payload) => Ok(payload),
            KWinScriptMessage::Error(message) => Err(anyhow!("KWin task script failed: {message}")),
        }
    }
    .await;

    let _ = script_proxy.call::<_, _, ()>("stop", &()).await;
    let _ = scripting_proxy
        .call::<_, _, ()>("unloadScript", &(script_name.as_str(),))
        .await;
    let _ = fs::remove_file(script_path);

    result
}

fn kwin_script(service: &str, body: &str) -> Result<String> {
    Ok(format!(
        r#"
function ge_result(payload) {{
    callDBus({service}, {path}, {interface}, "Result", String(payload));
}}

function ge_error(payload) {{
    callDBus({service}, {path}, {interface}, "Error", String(payload));
}}

function ge_windows() {{
    if (typeof workspace.windowList === "function") {{
        return workspace.windowList();
    }}
    if (workspace.stackingOrder) {{
        return workspace.stackingOrder;
    }}
    return [];
}}

try {{
{body}
}} catch (error) {{
    ge_error(String(error && error.stack ? error.stack : error));
}}
"#,
        service = js_string(service)?,
        path = js_string(KWIN_BRIDGE_PATH)?,
        interface = js_string(KWIN_BRIDGE_INTERFACE)?,
        body = body
    ))
}

fn kwin_focus_script(task_id: &str) -> String {
    format!(
        r#"
const targetId = {task_id};
const windows = ge_windows();
let target = null;

for (let i = 0; i < windows.length; i++) {{
    const w = windows[i];
    if (String(w.internalId) === targetId) {{
        target = w;
        break;
    }}
}}

if (target === null) {{
    ge_error("window not found");
}} else {{
    target.minimized = false;
    workspace.activeWindow = target;
    workspace.raiseWindow(target);
    ge_result("ok");
}}
"#,
        task_id = js_string(task_id).unwrap_or_else(|_| "\"\"".to_string())
    )
}

fn write_kwin_script(contents: &str) -> Result<PathBuf> {
    let file_name = format!(
        "gameease-kwin-{}-{}.js",
        process::id(),
        unix_time_nanos().unwrap_or_default()
    );
    let path = env::temp_dir().join(file_name);
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn js_string(value: &str) -> Result<String> {
    serde_json::to_string(value).context("failed to quote JavaScript string")
}

fn unix_time_nanos() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn is_kde_session() -> bool {
    env_contains("XDG_CURRENT_DESKTOP", "KDE")
        || env_contains("XDG_CURRENT_DESKTOP", "PLASMA")
        || env_contains("DESKTOP_SESSION", "plasma")
        || env::var_os("KDE_SESSION_VERSION").is_some()
}

fn env_contains(key: &str, needle: &str) -> bool {
    env::var(key)
        .map(|value| value.to_uppercase().contains(&needle.to_uppercase()))
        .unwrap_or(false)
}

fn discover_hyprland_signature() -> Option<String> {
    discover_hyprland_signature_from_runtime()
        .or_else(discover_hyprland_signature_from_hyprctl_instances)
}

fn discover_hyprland_signature_from_runtime() -> Option<String> {
    let hypr_dir = runtime_dir()?.join("hypr");
    let mut candidates = Vec::new();

    for entry in fs::read_dir(hypr_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.join(".socket.sock").exists() {
            continue;
        }

        let signature = entry.file_name().to_string_lossy().to_string();
        let modified = modified_secs(&path);
        candidates.push((modified, signature));
    }

    candidates
        .into_iter()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, signature)| signature)
}

fn discover_hyprland_signature_from_hyprctl_instances() -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["instances", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    let instances = hyprland_instances_from_json(&value);
    instances
        .into_iter()
        .filter_map(|instance| {
            let signature = json_string_field(instance, &["instance", "signature", "id"])?;
            let rank = json_i64_field(instance, &["time", "created", "pid"]);
            Some((rank, signature))
        })
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, signature)| signature)
}

fn hyprland_instances_from_json(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    if let Some(instances) = value.as_array() {
        instances.iter().collect()
    } else if let Some(instances) = value.get("instances").and_then(serde_json::Value::as_array) {
        instances.iter().collect()
    } else {
        vec![value]
    }
}

fn json_string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

fn json_i64_field(value: &serde_json::Value, names: &[&str]) -> i64 {
    names
        .iter()
        .find_map(|name| {
            let field = value.get(*name)?;
            field
                .as_i64()
                .or_else(|| field.as_u64().and_then(|number| i64::try_from(number).ok()))
                .or_else(|| field.as_str().and_then(|number| number.parse().ok()))
        })
        .unwrap_or(0)
}

fn discover_sway_socket() -> Option<String> {
    let runtime_dir = runtime_dir()?;
    let mut candidates = Vec::new();

    for entry in fs::read_dir(&runtime_dir).ok()?.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !file_name.starts_with("sway-ipc.") || !file_name.ends_with(".sock") {
            continue;
        }

        let path = entry.path();
        let modified = modified_secs(&path);
        candidates.push((modified, path.to_string_lossy().to_string()));
    }

    candidates
        .into_iter()
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, socket)| socket)
}

fn runtime_dir() -> Option<PathBuf> {
    env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
}

fn modified_secs(path: &PathBuf) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
