import { chromium, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { createInterface } from "node:readline";

const root = process.cwd();
await mkdir("test-results", { recursive: true });
const server = spawn("python", ["-X", "utf8", "scripts/native_server.py"], {
  cwd: root,
  windowsHide: true,
});
const fixture = await new Promise((resolve, reject) => {
  const lines = createInterface({ input: server.stdout });
  lines.on("line", (line) => {
    if (line.startsWith("{")) resolve(JSON.parse(line));
  });
  server.on("error", reject);
});
const data = path.join(root, ".local", `native-state-${Date.now()}`);
const port = 19430;
const exe =
  process.env.M3U8_TEST_EXE || path.join(root, "target/debug/m3u8-studio.exe");
const app = spawn(exe, [], {
  cwd: root,
  env: {
    ...process.env,
    ...(process.env.M3U8_TEST_APP_PATH ? { PATH: process.env.M3U8_TEST_APP_PATH, Path: process.env.M3U8_TEST_APP_PATH } : {}),
    M3U8_STUDIO_DATA_DIR: data,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --autoplay-policy=no-user-gesture-required`,
  },
  windowsHide: true,
});
let browser;
const errors = [];
const metrics = {};
const start = Date.now();
try {
  for (let n = 0; n < 60; n++) {
    try {
      browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
      break;
    } catch {
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  if (!browser)
    throw new Error("Native WebView debugging endpoint did not start");
  const context = browser.contexts()[0];
  let page = context.pages()[0];
  if (!page) page = await context.waitForEvent("page");
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("dialog", (dialog) => dialog.accept());
  await expect(page.getByRole("heading", { name: "下载工作台" })).toBeVisible({
    timeout: 30000,
  });
  metrics.startup_ms = Date.now() - start;
  if (await page.getByText("我已了解并同意合理使用本工具").isVisible()) {
    await page.getByText("我已了解并同意合理使用本工具").click();
    await page.getByRole("button", { name: "进入工作台" }).click();
  }
  const invoke = (command, args = {}) =>
    page.evaluate(
      ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args),
      { command, args },
    );
  const settings = await invoke("get_settings");
  settings.notify = false;
  settings.low_disk_mib = 0;
  await invoke("save_settings", { settings });
  await page.reload();
  await expect(page.getByRole("heading", { name: "下载工作台" })).toBeVisible();
  await page.getByRole("button", { name: "新建任务", exact: true }).click();
  await page
    .getByPlaceholder("https://example.com/video/index.m3u8")
    .fill(fixture.url + "/plain/index.m3u8");
  await page.getByRole("button", { name: "解析", exact: true }).click();
  await expect(page.getByText("8 个分片", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "开始下载", exact: true }).click();
  await expect
    .poll(
      async () => {
        const tasks = await invoke("list_tasks");
        return tasks[0]?.state;
      },
      { timeout: 60000 },
    )
    .toBe("completed");
  const tasks = await invoke("list_tasks");
  const downloaded = tasks[0];
  expect(downloaded.output).toBeTruthy();
  await expect(page.locator(".task-row")).toHaveCount(1);
  await page.screenshot({
    path: "test-results/native-workspace.png",
    fullPage: true,
  });
  const taskDetails = await invoke("task_details", {
    id: downloaded.id,
    offset: 0,
    limit: 20,
  });
  expect(taskDetails.total).toBe(8);
  expect(taskDetails.attempts.length).toBe(1);
  const lifecycle = await invoke("task_input", { id: downloaded.id });
  lifecycle.title = "Lifecycle test";
  lifecycle.network.limit_kib = 64;
  lifecycle.keep_cache = true;
  const pausing = await invoke("add_task", { input: lifecycle, start: true });
  await expect
    .poll(
      async () =>
        (await invoke("list_tasks")).find((t) => t.id === pausing.id)?.state,
    )
    .toBe("downloading");
  const pauseStart = Date.now();
  await invoke("task_action", { id: pausing.id, action: "pause" });
  metrics.pause_ms = Date.now() - pauseStart;
  expect(metrics.pause_ms).toBeLessThan(3000);
  expect(
    (await invoke("list_tasks")).find((t) => t.id === pausing.id).state,
  ).toBe("paused");
  await Promise.all([
    invoke("task_action", { id: pausing.id, action: "start" }),
    invoke("task_action", { id: pausing.id, action: "start" }),
  ]);
  await expect
    .poll(
      async () =>
        (await invoke("list_tasks")).find((t) => t.id === pausing.id)?.state,
    )
    .toBe("downloading");
  const cancelStart = Date.now();
  await invoke("task_action", { id: pausing.id, action: "cancel" });
  metrics.cancel_ms = Date.now() - cancelStart;
  expect(metrics.cancel_ms).toBeLessThan(3000);
  const cancelled = (await invoke("list_tasks")).find(
    (t) => t.id === pausing.id,
  );
  expect(cancelled.state).toBe("cancelled");
  expect(cancelled.output).toBeNull();
  await invoke("task_action", { id: pausing.id, action: "delete_cache" });
  lifecycle.source = fixture.url + "/gap.m3u8";
  lifecycle.network.limit_kib = 0;
  lifecycle.allow_gaps = true;
  lifecycle.confirm_gaps = false;
  const gap = await invoke("add_task", { input: lifecycle, start: true });
  await expect
    .poll(
      async () =>
        (await invoke("list_tasks")).find((t) => t.id === gap.id)?.state,
      { timeout: 20000 },
    )
    .toBe("waiting");
  const waiting = (await invoke("list_tasks")).find((t) => t.id === gap.id);
  expect(waiting.error.code).toBe("MISSING_CONFIRMATION");
  expect(waiting.output).toBeNull();
  await invoke("task_action", { id: gap.id, action: "confirm_gaps" });
  await expect
    .poll(
      async () =>
        (await invoke("list_tasks")).find((t) => t.id === gap.id)?.state,
      { timeout: 20000 },
    )
    .toBe("completed_with_gaps");
  await invoke("task_action", { id: gap.id, action: "delete_cache" });

  // Authenticated AES preview is initiated from the actual form and passes through IPC.
  await page.getByRole("button", { name: "新建任务", exact: true }).click();
  await page
    .getByPlaceholder("https://example.com/video/index.m3u8")
    .fill(fixture.url + "/protected/encrypted/index.m3u8");
  await page.getByText("请求、代理与解密", { exact: true }).click();
  await page
    .getByLabel("请求头 JSON（Cookie、Referer 等）", { exact: true })
    .fill('{"Cookie":"session=acceptance-secret"}');
  await page.getByRole("button", { name: "解析", exact: true }).click();
  await expect(page.getByText("8 个分片", { exact: true })).toBeVisible();
  const previewStart = Date.now();
  await page.getByRole("button", { name: "在线预览", exact: true }).click();
  await expect
    .poll(
      () =>
        page
          .locator("video")
          .evaluate((v) => ({ time: v.currentTime, width: v.videoWidth })),
      { timeout: 30000 },
    )
    .toMatchObject({ width: 320 });
  await expect
    .poll(() => page.locator("video").evaluate((v) => v.currentTime), {
      timeout: 15000,
    })
    .toBeGreaterThan(0.2);
  metrics.preview_first_frame_ms = Date.now() - previewStart;
  await page.locator("video").evaluate((v) => {
    v.pause();
    v.volume = 0.4;
    v.currentTime = 4;
  });
  await expect
    .poll(() => page.locator("video").evaluate((v) => v.currentTime))
    .toBeGreaterThanOrEqual(4);
  await page.screenshot({ path: "test-results/native-preview.png" });
  await page.locator(".player-dialog .dialog-header button").click();
  await page.locator(".dialog.large .dialog-header button").click();

  // Local TS is remuxed to a temporary MP4, then served by the scoped asset protocol.
  const id = crypto.randomUUID();
  const ts = path.join(fixture.root, "plain/segment-100.ts");
  const local = await invoke("prepare_local_media", { id, path: ts });
  expect(local.endsWith(".mp4")).toBeTruthy();
  const localResult = await page.evaluate(async (local) => {
    const convertFileSrc = (path) =>
      window.__TAURI_INTERNALS__.convertFileSrc(path, "asset");
    const video = document.createElement("video");
    document.body.append(video);
    video.src = convertFileSrc(local);
    video.muted = true;
    try {
      return await new Promise((resolve, reject) => {
        video.onloadeddata = () =>
          resolve({ width: video.videoWidth, duration: video.duration });
        video.onerror = () =>
          reject(
            new Error("Local TS playback failed: " + video.error?.message),
          );
        setTimeout(() => reject(new Error("Local media timed out")), 10000);
      });
    } finally {
      video.remove();
    }
  }, local);
  expect(localResult.width).toBe(320);
  await invoke("close_preview", { id });
  expect(errors).toEqual([]);
  metrics.result = "passed";
  metrics.tests = [
    "native form download + events",
    "authenticated AES preview + seek",
    "local TS asset playback",
    "segment/attempt details; pause, duplicate start, cancel and delete",
    "missing segments require confirmation before output",
  ];
  metrics.data_dir = data;
  await writeFile(
    "test-results/native-metrics.json",
    JSON.stringify(metrics, null, 2),
  );
  console.log(JSON.stringify(metrics, null, 2));
} finally {
  if (browser) await browser.close();
  app.kill();
  server.kill();
}
