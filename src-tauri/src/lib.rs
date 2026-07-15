use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::Duration,
};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

const APP_PORT: u16 = 14300;
const PLUGIN_NAME: &str = "@tommyfgj/plugin-data-source-readonly-mysql";
const ADDRESS_BAR_SCRIPT: &str = r#"
(() => {
  const id = 'nocobase-desktop-address-bar';
  document.getElementById(id)?.remove();
  const bar = document.createElement('div');
  bar.id = id;
  bar.style.cssText = 'position:fixed;top:0;left:0;right:0;height:42px;z-index:2147483647;display:flex;align-items:center;gap:8px;padding:6px 10px;background:#f3f4f7;border-bottom:1px solid #d8dce3;font:13px -apple-system,BlinkMacSystemFont,sans-serif;';
  const input = document.createElement('input');
  input.value = location.href;
  input.setAttribute('aria-label', '地址');
  input.style.cssText = 'flex:1;height:28px;border:1px solid #c9ced8;border-radius:7px;padding:0 10px;background:white;color:#252a33;outline:none;';
  input.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') location.href = input.value;
  });
  const copy = document.createElement('button');
  copy.textContent = '复制';
  copy.style.cssText = 'height:28px;border:1px solid #c9ced8;border-radius:7px;padding:0 12px;background:white;color:#353b47;cursor:pointer;';
  copy.addEventListener('click', async () => {
    await navigator.clipboard.writeText(input.value);
    copy.textContent = '已复制';
    setTimeout(() => copy.textContent = '复制', 1000);
  });
  bar.append(input, copy);
  document.documentElement.style.paddingTop = '42px';
  document.documentElement.style.boxSizing = 'border-box';
  document.documentElement.style.height = '100%';
  document.body.style.height = 'calc(100% - 42px)';
  document.body.appendChild(bar);
})();
"#;

#[derive(Default)]
struct RuntimeState {
    child: Mutex<Option<Child>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DatabaseConfig {
    dialect: String,
    host: String,
    port: u16,
    database: String,
    user: String,
    password: String,
    app_key: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            dialect: "postgres".into(),
            host: "127.0.0.1".into(),
            port: 5432,
            database: "nocobase".into(),
            user: "nocobase".into(),
            password: String::new(),
            app_key: Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    running: bool,
    url: String,
    runtime_ready: bool,
    log_path: String,
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|error| error.to_string())
}

fn runtime_dir(_app: &AppHandle) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("无法确定 HOME 目录")?;
    Ok(PathBuf::from(home)
        .join(".nocobase-desktop")
        .join("runtime"))
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("database.json"))
}

fn log_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("logs").join("nocobase.log"))
}

fn save_config_file(app: &AppHandle, config: &DatabaseConfig) -> Result<(), String> {
    let path = config_path(app)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    fs::write(
        &path,
        serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn load_config_file(app: &AppHandle) -> Result<DatabaseConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(DatabaseConfig::default());
    }
    serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

fn bundled_resource(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let root = app
        .path()
        .resource_dir()
        .map_err(|error| error.to_string())?;
    let nested = root.join("resources").join(name);
    if nested.exists() {
        Ok(nested)
    } else {
        Ok(root.join(name))
    }
}

fn ensure_runtime(app: &AppHandle) -> Result<PathBuf, String> {
    let runtime = runtime_dir(app)?;
    let marker = runtime.join(".desktop-runtime-ready");
    let current_version = env!("CARGO_PKG_VERSION");
    if fs::read_to_string(&marker)
        .map(|version| version.trim() == current_version)
        .unwrap_or(false)
    {
        return Ok(runtime);
    }

    let initialized = runtime.join(".nocobase-initialized").exists();
    let nb_initialized = runtime.join(".nb-desktop-initialized").exists();
    let storage = runtime.join("storage");
    let storage_backup = runtime
        .parent()
        .ok_or_else(|| "运行时目录没有父目录".to_string())?
        .join(format!(".storage-upgrade-{}", Uuid::new_v4()));
    if storage.exists() {
        fs::rename(&storage, &storage_backup).map_err(|error| error.to_string())?;
    }
    if runtime.exists() {
        fs::remove_dir_all(&runtime).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&runtime).map_err(|error| error.to_string())?;
    let archive =
        File::open(bundled_resource(app, "runtime.tar.gz")?).map_err(|error| error.to_string())?;
    tar::Archive::new(GzDecoder::new(archive))
        .unpack(&runtime)
        .map_err(|error| error.to_string())?;
    if storage_backup.exists() {
        let extracted_storage = runtime.join("storage");
        if extracted_storage.exists() {
            fs::remove_dir_all(&extracted_storage).map_err(|error| error.to_string())?;
        }
        fs::rename(&storage_backup, &extracted_storage).map_err(|error| error.to_string())?;
    }
    if initialized {
        fs::write(runtime.join(".nocobase-initialized"), "ok")
            .map_err(|error| error.to_string())?;
    }
    if nb_initialized {
        fs::write(runtime.join(".nb-desktop-initialized"), "ok")
            .map_err(|error| error.to_string())?;
    }
    fs::write(marker, current_version).map_err(|error| error.to_string())?;
    Ok(runtime)
}

fn command_env(command: &mut Command, runtime: &Path, config: &DatabaseConfig) {
    command
        .env("APP_ENV", "development")
        .env("APP_HOST", "127.0.0.1")
        .env("APP_PORT", APP_PORT.to_string())
        .env("API_BASE_PATH", "/api/")
        .env("APP_KEY", &config.app_key)
        .env("APP_LAUNCH_MODE", "node")
        .env("APP_PACKAGE_ROOT", "node_modules/@nocobase/app")
        .env("NODE_MODULES_PATH", runtime.join("node_modules"))
        .env("STORAGE_PATH", runtime.join("storage"))
        .env("DB_DIALECT", &config.dialect)
        .env("DB_HOST", &config.host)
        .env("DB_PORT", config.port.to_string())
        .env("DB_DATABASE", &config.database)
        .env("DB_USER", &config.user)
        .env("DB_PASSWORD", &config.password)
        .env(
            "PLUGIN_PACKAGE_PREFIX",
            "@nocobase/plugin-,@nocobase/preset-,@tommyfgj/plugin-",
        );
    if matches!(config.dialect.as_str(), "mysql" | "mariadb") {
        command.env("DB_UNDERSCORED", "true");
    }
}

#[cfg(unix)]
fn login_shell_path() -> Option<String> {
    const MARKER: &str = "__NOCOBASE_DESKTOP_PATH__";
    let output = Command::new("/bin/zsh")
        .args(["-lc", "printf '__NOCOBASE_DESKTOP_PATH__%s' \"$PATH\""])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let marker_index = stdout.rfind(MARKER)?;
    let path = stdout[marker_index + MARKER.len()..].trim();
    (!path.is_empty()).then(|| path.to_string())
}

fn node_command(
    app: &AppHandle,
    runtime: &Path,
    config: &DatabaseConfig,
) -> Result<Command, String> {
    let mut command = Command::new(bundled_resource(app, "node")?);
    command.current_dir(runtime);
    #[cfg(unix)]
    if let Some(path) = login_shell_path() {
        command.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command_env(&mut command, runtime, config);
    Ok(command)
}

fn terminate_child(child: &mut Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
        for _ in 0..80 {
            if child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_some()
            {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGTERM);
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        child.wait().map_err(|error| error.to_string())?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        child.kill().map_err(|error| error.to_string())?;
        child.wait().map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn append_log(app: &AppHandle) -> Result<File, String> {
    let path = log_path(app)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())
}

fn run_setup_command(
    app: &AppHandle,
    runtime: &Path,
    config: &DatabaseConfig,
    args: &[&str],
) -> Result<(), String> {
    let mut command = node_command(app, runtime, config)?;
    let log = append_log(app)?;
    let status = command
        .arg("node_modules/@nocobase/cli-v1/bin/index.js")
        .args(args)
        .stdout(Stdio::from(
            log.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log))
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("NocoBase 命令执行失败：{}", args.join(" ")))
    }
}

fn wait_for_api() -> Result<(), String> {
    let address = SocketAddr::from(([127, 0, 0, 1], APP_PORT));
    for _ in 0..300 {
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(300)) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = stream.write_all(
                b"GET /api/app:getLang HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            );
            let mut response = String::new();
            if stream.read_to_string(&mut response).is_ok()
                && response.starts_with("HTTP/1.1 200")
                && response.contains("application/json")
                && !response.contains("APP_COMMANDING")
                && !response.contains("\"maintaining\":true")
            {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err("NocoBase 在 5 分钟内未完成数据源加载，请查看运行日志".into())
}

fn install_nb_launcher(app: &AppHandle, runtime: &Path) -> Result<PathBuf, String> {
    let node = bundled_resource(app, "node")?;
    let home = std::env::var_os("HOME").ok_or("无法确定 HOME 目录")?;
    let bin_dir = PathBuf::from(home).join(".nocobase-desktop").join("bin");
    fs::create_dir_all(&bin_dir).map_err(|error| error.to_string())?;
    let script = bin_dir.join("nb");
    let body = format!(
        "#!/bin/sh\ncd '{}'\nexec '{}' 'node_modules/@nocobase/cli/bin/run.js' \"$@\"\n",
        runtime.display(),
        node.display()
    );
    fs::write(&script, body).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }
    Ok(script)
}

fn initialize_nb_environment(
    app: &AppHandle,
    runtime: &Path,
    config: &DatabaseConfig,
) -> Result<(), String> {
    let marker = runtime.join(".nb-desktop-initialized");
    if marker.exists() {
        return Ok(());
    }
    install_nb_launcher(app, runtime)?;
    let mut command = node_command(app, runtime, config)?;
    let log = append_log(app)?;
    let status = command
        .arg("node_modules/@nocobase/cli/bin/run.js")
        .args([
            "init",
            "--env",
            "desktop",
            "--yes",
            "--setup-mode",
            "connect-remote",
            "--api-base-url",
            "http://127.0.0.1:14300/api",
            "--auth-type",
            "basic",
            "--username",
            "nocobase",
            "--password",
            "admin123",
            "--skip-skills",
            "--force",
        ])
        .stdout(Stdio::from(
            log.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log))
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("nb desktop 环境注册失败".into());
    }
    fs::write(marker, "ok").map_err(|error| error.to_string())
}

#[tauri::command]
fn get_database_config(app: AppHandle) -> Result<DatabaseConfig, String> {
    load_config_file(&app)
}

#[tauri::command]
fn save_database_config(app: AppHandle, config: DatabaseConfig) -> Result<(), String> {
    save_config_file(&app, &config)
}

#[tauri::command]
fn runtime_status(app: AppHandle, state: State<RuntimeState>) -> Result<RuntimeStatus, String> {
    let mut guard = state.child.lock().map_err(|error| error.to_string())?;
    let child_running = match guard.as_mut() {
        Some(child) => child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none(),
        None => false,
    };
    if !child_running {
        *guard = None;
    }
    let address = SocketAddr::from(([127, 0, 0, 1], APP_PORT));
    let reachable = TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok();
    Ok(RuntimeStatus {
        running: child_running || reachable,
        url: format!("http://127.0.0.1:{APP_PORT}"),
        runtime_ready: runtime_dir(&app)?.join(".desktop-runtime-ready").exists(),
        log_path: log_path(&app)?.display().to_string(),
    })
}

fn start_runtime_blocking(app: AppHandle, config: DatabaseConfig) -> Result<(), String> {
    let state = app.state::<RuntimeState>();
    save_config_file(&app, &config)?;
    let runtime = ensure_runtime(&app)?;
    let initialized = runtime.join(".nocobase-initialized");
    if !initialized.exists() {
        run_setup_command(&app, &runtime, &config, &["install"])?;
        run_setup_command(&app, &runtime, &config, &["pm", "enable", PLUGIN_NAME])?;
        fs::write(initialized, "ok").map_err(|error| error.to_string())?;
    }

    let mut guard = state.child.lock().map_err(|error| error.to_string())?;
    if let Some(child) = guard.as_mut() {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Ok(());
        }
    }

    let mut command = node_command(&app, &runtime, &config)?;
    let log = append_log(&app)?;
    let child = command
        .arg("node_modules/@nocobase/cli-v1/bin/index.js")
        .arg("start")
        .arg("--launch-mode")
        .arg("node")
        .arg("--port")
        .arg(APP_PORT.to_string())
        .stdout(Stdio::from(
            log.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| error.to_string())?;
    *guard = Some(child);
    drop(guard);
    wait_for_api()?;
    initialize_nb_environment(&app, &runtime, &config)?;
    Ok(())
}

#[tauri::command]
async fn start_runtime(app: AppHandle, config: DatabaseConfig) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || start_runtime_blocking(app, config))
        .await
        .map_err(|error| error.to_string())?
}

fn stop_runtime_blocking(app: AppHandle) -> Result<(), String> {
    let state = app.state::<RuntimeState>();
    let mut guard = state.child.lock().map_err(|error| error.to_string())?;
    if let Some(mut child) = guard.take() {
        terminate_child(&mut child)?;
    }
    Ok(())
}

#[tauri::command]
async fn stop_runtime(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || stop_runtime_blocking(app))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
fn open_nocobase(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("nocobase") {
        window
            .navigate(format!("http://127.0.0.1:{APP_PORT}").parse().unwrap())
            .map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }
    WebviewWindowBuilder::new(
        &app,
        "nocobase",
        WebviewUrl::External(format!("http://127.0.0.1:{APP_PORT}").parse().unwrap()),
    )
    .title("NocoBase")
    .inner_size(1280.0, 840.0)
    .on_page_load(|window, _| {
        let _ = window.eval(ADDRESS_BAR_SCRIPT);
    })
    .build()
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn install_nb_cli(app: AppHandle) -> Result<String, String> {
    let runtime = ensure_runtime(&app)?;
    Ok(install_nb_launcher(&app, &runtime)?.display().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(RuntimeState::default())
        .invoke_handler(tauri::generate_handler![
            get_database_config,
            save_database_config,
            runtime_status,
            start_runtime,
            stop_runtime,
            open_nocobase,
            install_nb_cli
        ])
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                let app = window.app_handle().clone();
                let state = window.state::<RuntimeState>();
                if let Ok(mut guard) = state.child.lock() {
                    if let Some(mut child) = guard.take() {
                        let _ = terminate_child(&mut child);
                    }
                };
                app.exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
