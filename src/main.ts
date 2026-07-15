import { invoke } from "@tauri-apps/api/core";

type DatabaseConfig = {
  dialect: string;
  host: string;
  port: number;
  database: string;
  user: string;
  password: string;
  appKey: string;
};

type RuntimeStatus = {
  running: boolean;
  url: string;
  runtimeReady: boolean;
  logPath: string;
};

const byId = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;

function showMessage(message: string, error = false) {
  const element = byId<HTMLDivElement>("message");
  element.textContent = message;
  element.className = error ? "error" : "success";
}

function setBusy(busy: boolean) {
  byId<HTMLButtonElement>("start").disabled = busy;
  byId<HTMLButtonElement>("start").textContent = busy
    ? "正在初始化…"
    : "保存并启动";
}

let progressTimer: number | undefined;

function startProgress(phases: string[]) {
  let index = 0;
  const panel = byId<HTMLDivElement>("startup-progress");
  const label = byId<HTMLSpanElement>("progress-label");
  panel.hidden = false;
  label.textContent = phases[index];
  progressTimer = window.setInterval(() => {
    index = Math.min(index + 1, phases.length - 1);
    label.textContent = phases[index];
  }, 4000);
}

function stopProgress(success: boolean, completedLabel = "启动完成") {
  if (progressTimer !== undefined) {
    window.clearInterval(progressTimer);
    progressTimer = undefined;
  }
  const panel = byId<HTMLDivElement>("startup-progress");
  if (success) {
    byId<HTMLSpanElement>("progress-label").textContent = completedLabel;
    window.setTimeout(() => {
      panel.hidden = true;
    }, 900);
  } else {
    panel.hidden = true;
  }
}

async function waitForPaint() {
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

function readForm(appKey: string): DatabaseConfig {
  return {
    dialect: byId<HTMLSelectElement>("dialect").value,
    host: byId<HTMLInputElement>("host").value.trim(),
    port: Number(byId<HTMLInputElement>("port").value),
    database: byId<HTMLInputElement>("database").value.trim(),
    user: byId<HTMLInputElement>("user").value.trim(),
    password: byId<HTMLInputElement>("password").value,
    appKey,
  };
}

function fillForm(config: DatabaseConfig) {
  byId<HTMLSelectElement>("dialect").value = config.dialect;
  byId<HTMLInputElement>("host").value = config.host;
  byId<HTMLInputElement>("port").value = String(config.port);
  byId<HTMLInputElement>("database").value = config.database;
  byId<HTMLInputElement>("user").value = config.user;
  byId<HTMLInputElement>("password").value = config.password;
}

async function updateStatus() {
  const status = await invoke<RuntimeStatus>("runtime_status");
  const pill = byId<HTMLSpanElement>("status-pill");
  pill.textContent = status.running ? "运行中" : "未启动";
  pill.className = `pill ${status.running ? "online" : ""}`;
  byId<HTMLButtonElement>("open").disabled = !status.running;
  byId<HTMLButtonElement>("stop").disabled = !status.running;
  return status;
}

window.addEventListener("DOMContentLoaded", async () => {
  let config = await invoke<DatabaseConfig>("get_database_config");
  fillForm(config);
  await updateStatus();

  byId<HTMLFormElement>("database-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    config = readForm(config.appKey);
    setBusy(true);
    startProgress([
      "正在准备运行环境…",
      "正在检查数据库配置…",
      "正在初始化 NocoBase…",
      "正在启动本地服务…",
      "正在注册 nb 命令环境…",
    ]);
    showMessage("首次启动会解压运行时并初始化数据库，请稍候。");
    await waitForPaint();
    let succeeded = false;
    try {
      await invoke("start_runtime", { config });
      for (let attempt = 0; attempt < 60; attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, 1000));
        const status = await updateStatus();
        if (status.running) {
          succeeded = true;
          showMessage(`NocoBase 已启动：${status.url}`);
          await invoke("open_nocobase");
          return;
        }
      }
      throw new Error("服务启动超时，请查看日志。");
    } catch (error) {
      showMessage(String(error), true);
    } finally {
      stopProgress(succeeded);
      setBusy(false);
    }
  });

  byId<HTMLButtonElement>("stop").addEventListener("click", async () => {
    const stopButton = byId<HTMLButtonElement>("stop");
    const startButton = byId<HTMLButtonElement>("start");
    stopButton.disabled = true;
    startButton.disabled = true;
    startProgress([
      "正在通知 NocoBase 停止服务…",
      "正在等待任务与连接关闭…",
      "正在清理后台进程…",
    ]);
    await waitForPaint();
    let succeeded = false;
    try {
      await invoke("stop_runtime");
      await updateStatus();
      succeeded = true;
      showMessage("NocoBase 已停止。");
    } catch (error) {
      showMessage(String(error), true);
    } finally {
      stopProgress(succeeded, "停止完成");
      startButton.disabled = false;
      if (!succeeded) {
        stopButton.disabled = false;
      }
    }
  });

  byId<HTMLButtonElement>("open").addEventListener("click", async () => {
    await invoke("open_nocobase");
  });

  byId<HTMLButtonElement>("install-cli").addEventListener("click", async () => {
    try {
      const path = await invoke<string>("install_nb_cli");
      showMessage(`nb 已安装到 ${path}。请将 ~/.nocobase-desktop/bin 加入 PATH。`);
    } catch (error) {
      showMessage(String(error), true);
    }
  });
});
