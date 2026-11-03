<script setup lang="ts">
import {
  ref,
  computed,
  onMounted,
  onBeforeUnmount,
  watch,
  nextTick,
} from "vue";
import { invoke, convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import {
  ArrowDownToLine,
  Plus,
  Search,
  Settings2,
  FolderOpen,
  Play,
  Pause,
  Square,
  Trash2,
  RefreshCw,
  Check,
  CheckCheck,
  Clock3,
  AlertCircle,
  ChevronRight,
  X,
  Film,
  Link,
  Layers,
  FileText,
  Sun,
  Moon,
  ShieldCheck,
  Copy,
  LoaderCircle,
  ArrowUpRight,
  Download,
  SlidersHorizontal,
  MonitorPlay,
  HardDrive,
  Languages,
  Clipboard,
} from "lucide-vue-next";
import type {
  Task,
  TaskInput,
  Settings,
  Playlist,
  Diagnostics,
  TaskDetails,
} from "./types";
import { inputDefaults, networkDefaults } from "./types";
import type Hls from "hls.js";
import "./style.css";

const clone = <T,>(value: T): T => JSON.parse(JSON.stringify(value));
const tasks = ref<Task[]>([]),
  selected = ref<string[]>([]),
  filter = ref("all"),
  query = ref(""),
  busy = ref(false),
  modal = ref(""),
  editing = ref<string | null>(null),
  input = ref<TaskInput>(inputDefaults()),
  parsed = ref<Playlist | null>(null),
  headerText = ref("{}"),
  originHeaderText = ref("{}"),
  keysText = ref("{}"),
  ivsText = ref("{}"),
  batch = ref(""),
  batchMode = ref(false),
  showSource = ref(false),
  selectedTask = ref<string | null>(null),
  toast = ref(""),
  pageError = ref(""),
  logs = ref(""),
  logQuery = ref(""),
  acceptNotice = ref(false),
  clipboard = ref(false),
  playerVisible = ref(false),
  playerError = ref(""),
  playerTitle = ref(""),
  subtitleUrl = ref("");
const settings = ref<Settings>({
  output_dir: "",
  cache_dir: "",
  theme: "system",
  language: "zh-CN",
  network: networkDefaults(),
  max_tasks: 2,
  max_requests: 16,
  max_processing: 1,
  low_disk_mib: 1024,
  keep_cache: false,
  notify: true,
  history_days: 0,
  log_level: "INFO",
  schema_version: 1,
  ffmpeg: "ffmpeg",
  ffprobe: "ffprobe",
});
const settingsDraft = ref<Settings>(clone(settings.value)),
  diagnostics = ref<Diagnostics | null>(null),
  video = ref<HTMLVideoElement | null>(null);
let unlisten: UnlistenFn | undefined,
  player: Hls | null = null,
  previewId = "",
  toastTimer: ReturnType<typeof setTimeout> | undefined,
  poll: ReturnType<typeof setInterval> | undefined,
  lastClipboard = "";
const en = computed(() => settings.value.language === "en");
const t = (zh: string, english: string) => (en.value ? english : zh);
const running = (task: Task) =>
  ["queued", "parsing", "downloading", "processing", "pausing"].includes(
    task.state,
  );
const complete = (task: Task) =>
  ["completed", "completed_with_gaps"].includes(task.state);
const activeCount = computed(() => tasks.value.filter(running).length),
  completeCount = computed(() => tasks.value.filter(complete).length),
  failedCount = computed(
    () =>
      tasks.value.filter((x) => ["failed", "waiting"].includes(x.state)).length,
  );
const visibleTasks = computed(() =>
  tasks.value.filter(
    (x) =>
      (filter.value === "all" ||
        (filter.value === "active" && running(x)) ||
        (filter.value === "completed" && complete(x)) ||
        (filter.value === "failed" &&
          ["failed", "waiting"].includes(x.state))) &&
      x.title.toLowerCase().includes(query.value.toLowerCase()),
  ),
);
const focusTask = computed(() =>
  tasks.value.find((x) => x.id === selectedTask.value),
);
const speed = computed(() => tasks.value.reduce((n, x) => n + x.speed, 0));
const formatBytes = (n: number) =>
  n < 1024
    ? `${n} B`
    : n < 1048576
      ? `${(n / 1024).toFixed(1)} KiB`
      : n < 1073741824
        ? `${(n / 1048576).toFixed(1)} MiB`
        : `${(n / 1073741824).toFixed(2)} GiB`;
const stateLabel = (s: string) =>
  ({
    ready: t("待下载", "Ready"),
    queued: t("排队中", "Queued"),
    parsing: t("解析中", "Parsing"),
    downloading: t("下载中", "Downloading"),
    processing: t("处理中", "Processing"),
    pausing: t("暂停中", "Stopping"),
    paused: t("已暂停", "Paused"),
    cancelled: t("已取消", "Cancelled"),
    failed: t("失败", "Failed"),
    waiting: t("需要处理", "Needs attention"),
    completed: t("已完成", "Completed"),
    completed_with_gaps: t("有缺失", "Incomplete"),
  })[s] || s;
function notify(message: string) {
  toast.value = message;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toast.value = ""), 5000);
}
function message(error: unknown) {
  return typeof error === "object" && error && "message" in error
    ? String(error.message)
    : String(error);
}
async function guard(action: () => Promise<unknown>) {
  try {
    await action();
  } catch (e) {
    notify(message(e));
  }
}
async function refresh() {
  tasks.value = await invoke<Task[]>("list_tasks");
  selected.value = selected.value.filter((id) =>
    tasks.value.some((x) => x.id === id),
  );
}
function applyTheme() {
  const theme = settings.value.theme;
  document.documentElement.dataset.theme =
    theme === "system"
      ? matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : theme;
  document.documentElement.lang = en.value ? "en" : "zh-CN";
}
watch(() => [settings.value.theme, settings.value.language], applyTheme);
function acceptUsage() {
  localStorage.setItem("usage-notice", "accepted");
  modal.value = "";
}
function resetForm() {
  input.value = {
    ...inputDefaults(),
    output_dir: settings.value.output_dir,
    network: clone(settings.value.network),
    keep_cache: settings.value.keep_cache,
    ffmpeg: settings.value.ffmpeg,
    ffprobe: settings.value.ffprobe,
  };
  parsed.value = null;
  headerText.value = JSON.stringify(input.value.network.headers, null, 2);
  originHeaderText.value = JSON.stringify(
    input.value.network.origin_headers,
    null,
    2,
  );
  keysText.value = "{}";
  ivsText.value = "{}";
  batch.value = "";
  batchMode.value = false;
  editing.value = null;
  showSource.value = false;
  pageError.value = "";
}
const details = ref<TaskDetails | null>(null),
  detailOffset = ref(0),
  forceStart = ref(0),
  forceEnd = ref(0),
  defaultHeaders = ref("{}");
const safeUri = (uri: string) => {
  try {
    const u = new URL(uri);
    return `${u.protocol}//${u.host}/…`;
  } catch {
    return t("本地资源", "Local resource");
  }
};
async function loadDetails() {
  if (selectedTask.value)
    details.value = await invoke<TaskDetails>("task_details", {
      id: selectedTask.value,
      offset: detailOffset.value,
      limit: 20,
    });
}
watch(selectedTask, () => {
  detailOffset.value = 0;
  void guard(loadDetails);
});
async function connectionTest() {
  busy.value = true;
  try {
    const result = await invoke<{ elapsed_ms: number }>("test_connection", {
      input: formInput(),
    });
    notify(
      t(
        `连接成功 · ${result.elapsed_ms} ms`,
        `Connected · ${result.elapsed_ms} ms`,
      ),
    );
  } catch (e) {
    pageError.value = message(e);
  } finally {
    busy.value = false;
  }
}
async function clearHistory() {
  if (
    confirm(
      t(
        "清理全部已完成和已取消任务及分片缓存？正式输出会保留。",
        "Remove completed/cancelled records and segment caches? Output files will be kept.",
      ),
    )
  ) {
    await invoke("prune_history", { days: 0 });
    await refresh();
  }
}
function newTask() {
  resetForm();
  modal.value = "new";
}
function formInput(): TaskInput {
  const value = clone(input.value);
  if (value.force_key) value.force_range = [forceStart.value, forceEnd.value];
  else {
    value.force_range = null;
    value.force_key = null;
  }
  value.network.headers = JSON.parse(headerText.value);
  value.network.origin_headers = JSON.parse(originHeaderText.value);
  value.manual_keys = JSON.parse(keysText.value);
  value.manual_ivs = JSON.parse(ivsText.value);
  return value;
}
async function chooseDirectory(target: "input" | "output" | "cache") {
  const path = await open({ directory: true, multiple: false });
  if (typeof path === "string") {
    if (target === "input") input.value.output_dir = path;
    else if (target === "output") settingsDraft.value.output_dir = path;
    else settingsDraft.value.cache_dir = path;
  }
}
async function chooseSource() {
  const path = await open({
    multiple: false,
    filters: [{ name: "HLS Playlist", extensions: ["m3u8"] }],
  });
  if (typeof path === "string") {
    input.value.source = path;
    parsed.value = null;
  }
}
async function inspect() {
  busy.value = true;
  pageError.value = "";
  try {
    parsed.value = await invoke<Playlist>("inspect_source", {
      input: formInput(),
    });
    if (parsed.value.master) {
      let index = 0;
      parsed.value.variants.forEach((v, i) => {
        if (v.bandwidth > parsed.value!.variants[index].bandwidth) index = i;
      });
      input.value.variant = index;
    }
    notify(
      t(
        "解析完成，请确认轨道和输出设置",
        "Playlist ready. Review tracks and output.",
      ),
    );
  } catch (e) {
    pageError.value = message(e);
  } finally {
    busy.value = false;
  }
}
async function submit(start: boolean) {
  busy.value = true;
  pageError.value = "";
  try {
    const value = formInput();
    if (!editing.value) {
      const sources = batchMode.value
        ? batch.value
            .split(/\r?\n/)
            .map((x) => x.trim())
            .filter(Boolean)
        : [value.source];
      const duplicates = await invoke<string[]>("duplicate_sources", {
        sources,
      });
      if (
        duplicates.length &&
        !confirm(
          t(
            `发现 ${duplicates.length} 个重复来源，仍要创建独立任务吗？`,
            `Found ${duplicates.length} duplicate sources. Create separate tasks?`,
          ),
        )
      )
        return;
    }
    if (
      value.allow_gaps &&
      !confirm(
        t(
          "缺失分片可能造成画面或声音中断。确认允许生成标记为不完整的输出？",
          "Missing segments can cause gaps. Generate an explicitly incomplete output?",
        ),
      )
    )
      return;
    if (editing.value) {
      await invoke("update_task", { id: editing.value, input: value });
      if (start)
        await invoke("task_action", { id: editing.value, action: "start" });
    } else if (batchMode.value) {
      const lines = batch.value.split(/\r?\n/);
      let added = 0;
      const errors: string[] = [];
      for (let i = 0; i < lines.length; i++) {
        const source = lines[i].trim();
        if (!source) continue;
        if (added >= 1000) {
          errors.push("最多 1000 条");
          break;
        }
        try {
          await invoke("add_task", {
            input: {
              ...value,
              source,
              title: value.title ? `${value.title} ${i + 1}` : "",
            },
            start,
          });
          added++;
        } catch (e) {
          errors.push(`${i + 1}: ${message(e)}`);
        }
      }
      if (errors.length) {
        pageError.value = `已添加 ${added} 条；${errors.join("；")}`;
        await refresh();
        return;
      }
    } else {
      await invoke("add_task", { input: value, start });
    }
    modal.value = "";
    await refresh();
    notify(t("任务已保存", "Task saved"));
  } catch (e) {
    pageError.value = message(e);
  } finally {
    busy.value = false;
  }
}
async function action(task: Task, command: string) {
  await guard(async () => {
    await invoke("task_action", { id: task.id, action: command });
    await refresh();
  });
}
async function batchAction(command: string) {
  for (const id of selected.value) {
    const task = tasks.value.find((t) => t.id === id);
    if (task) await action(task, command);
  }
}
async function edit(task: Task, duplicate = false) {
  if (running(task)) {
    notify(t("请先暂停任务", "Pause the task first"));
    return;
  }
  resetForm();
  const saved = await invoke<TaskInput | null>("task_input", { id: task.id });
  if (saved) {
    input.value = saved;
    forceStart.value = saved.force_range?.[0] || 0;
    forceEnd.value = saved.force_range?.[1] || 0;
    headerText.value = JSON.stringify(saved.network.headers, null, 2);
    originHeaderText.value = JSON.stringify(
      saved.network.origin_headers,
      null,
      2,
    );
    keysText.value = JSON.stringify(saved.manual_keys, null, 2);
    ivsText.value = JSON.stringify(saved.manual_ivs, null, 2);
  }
  editing.value = duplicate ? null : task.id;
  if (duplicate) input.value.title += t(" 副本", " copy");
  modal.value = "new";
}
async function remove(task: Task) {
  if (
    confirm(
      t(
        `删除“${task.title}”的任务记录和分片缓存？已输出的文件将保留。`,
        `Delete “${task.title}” and its cache? Output files will be kept.`,
      ),
    )
  )
    await action(task, "delete_cache");
}
async function openSettings() {
  settingsDraft.value = clone(settings.value);
  defaultHeaders.value = JSON.stringify(
    settings.value.network.origin_headers,
    null,
    2,
  );
  modal.value = "settings";
  diagnostics.value = await invoke("diagnostics");
}
async function saveSettings() {
  busy.value = true;
  try {
    settingsDraft.value.network.origin_headers = JSON.parse(
      defaultHeaders.value,
    );
    await invoke("save_settings", { settings: settingsDraft.value });
    settings.value = clone(settingsDraft.value);
    modal.value = "";
    notify(t("设置已保存", "Settings saved"));
  } catch (e) {
    notify(message(e));
  } finally {
    busy.value = false;
  }
}
async function openLogs() {
  logs.value = await invoke<string>("read_logs");
  modal.value = "logs";
}
async function exportLogs() {
  const path = await save({
    defaultPath: "m3u8-diagnostics.jsonl",
    filters: [{ name: "JSON Lines", extensions: ["jsonl"] }],
  });
  if (path)
    await guard(async () => {
      await invoke("export_logs", { path });
      notify(t("已导出脱敏日志", "Sanitized logs exported"));
    });
}
async function closePlayer() {
  player?.destroy();
  player = null;
  video.value?.pause();
  video.value?.removeAttribute("src");
  video.value?.load();
  if (previewId) {
    await invoke("close_preview", { id: previewId }).catch(() => {});
    previewId = "";
  }
  playerVisible.value = false;
  subtitleUrl.value = "";
}
async function preview(inputValue?: TaskInput) {
  await closePlayer();
  busy.value = true;
  playerError.value = "";
  try {
    const value = inputValue || formInput();
    const result = await invoke<{ id: string; manifest: string }>(
      "open_preview",
      { input: value },
    );
    previewId = result.id;
    playerTitle.value = value.title || t("在线预览", "Stream preview");
    playerVisible.value = true;
    await nextTick();
    const { createPlayer } = await import("./player");
    if (video.value && previewId === result.id && playerVisible.value) {
      player = createPlayer(
        video.value,
        result.id,
        result.manifest,
        (err) => (playerError.value = err),
      );
    }
  } catch (e) {
    pageError.value = message(e);
    notify(message(e));
  } finally {
    busy.value = false;
  }
}
async function playTask(task: Task) {
  if (task.output) {
    await localPlayback(task.output, task.title);
  } else {
    const input = await invoke<TaskInput | null>("task_input", { id: task.id });
    if (input) await preview(input);
    else
      notify(
        t("请编辑任务，重新提供来源", "Edit this task to provide its source"),
      );
  }
}
async function localPlayback(path?: string, title?: string) {
  await closePlayer();
  if (!path) {
    const selected = await open({
      multiple: false,
      filters: [{ name: "Media", extensions: ["mp4", "ts", "mkv"] }],
    });
    if (typeof selected !== "string") return;
    path = selected;
  }
  previewId = crypto.randomUUID();
  const allowed = await invoke<string>("prepare_local_media", {
    id: previewId,
    path,
  });
  playerTitle.value = title || t("本地媒体", "Local media");
  playerError.value = "";
  playerVisible.value = true;
  await nextTick();
  if (video.value) {
    video.value.src = convertFileSrc(allowed);
    video.value.onerror = () =>
      (playerError.value = t(
        "此播放器不支持该媒体编码或容器，可在系统播放器中打开。",
        "This codec/container is not supported by the embedded player.",
      ));
    await video.value.play().catch(() => {});
  }
}
async function attachSubtitle() {
  const path = await open({
    multiple: false,
    filters: [{ name: "WebVTT", extensions: ["vtt"] }],
  });
  if (typeof path === "string") {
    subtitleUrl.value = convertFileSrc(
      await invoke<string>("allow_media", { path }),
    );
    await nextTick();
    if (video.value?.textTracks[0]) video.value.textTracks[0].mode = "showing";
  }
}
async function clipboardCheck() {
  if (!clipboard.value || !document.hasFocus() || modal.value) return;
  try {
    const text = (await navigator.clipboard.readText()).trim();
    if (text !== lastClipboard && /^https?:\/\//i.test(text)) {
      lastClipboard = text;
      if (
        confirm(
          t(
            "剪贴板中检测到链接，要添加到新任务吗？",
            "A link was found in the clipboard. Add a new task?",
          ),
        )
      ) {
        newTask();
        input.value.source = text;
      }
    }
  } catch {}
}
function toggleSelected(id: string) {
  selected.value = selected.value.includes(id)
    ? selected.value.filter((x) => x !== id)
    : [...selected.value, id];
}
function selectAll() {
  selected.value =
    selected.value.length === visibleTasks.value.length
      ? []
      : visibleTasks.value.map((x) => x.id);
}
const systemTheme = matchMedia("(prefers-color-scheme: dark)");
let previousFocus: HTMLElement | null = null;
watch([modal, playerVisible], async () => {
  if (modal.value || playerVisible.value) {
    previousFocus = document.activeElement as HTMLElement;
    await nextTick();
    const dialog = document.querySelector<HTMLElement>(
      playerVisible.value ? ".player-dialog" : '[role="dialog"]',
    );
    dialog
      ?.querySelector<HTMLElement>(
        "input:not([disabled]),button:not([disabled]),select,textarea",
      )
      ?.focus();
  } else {
    previousFocus?.focus();
  }
});
function dialogKeys(event: KeyboardEvent) {
  if (!modal.value && !playerVisible.value) return;
  if (event.key === "Escape" && modal.value !== "notice") {
    if (playerVisible.value) void closePlayer();
    else modal.value = "";
    event.preventDefault();
    return;
  }
  if (event.key !== "Tab") return;
  const dialog = document.querySelector<HTMLElement>(
    playerVisible.value ? ".player-dialog" : '[role="dialog"]',
  );
  const focusable = Array.from(
    dialog?.querySelectorAll<HTMLElement>(
      'button:not([disabled]),input:not([disabled]),select,textarea,video,[tabindex="0"]',
    ) || [],
  ).filter((element) => element.offsetParent !== null);
  const first = focusable[0],
    last = focusable.at(-1);
  if (event.shiftKey && document.activeElement === first) {
    last?.focus();
    event.preventDefault();
  } else if (!event.shiftKey && document.activeElement === last) {
    first?.focus();
    event.preventDefault();
  }
}
onMounted(async () => {
  document.addEventListener("keydown", dialogKeys);
  systemTheme.addEventListener("change", applyTheme);
  applyTheme();
  if (!isTauri()) {
    pageError.value = t(
      "浏览器预览仅展示界面。请通过桌面应用执行下载。",
      "Use the desktop application to download media.",
    );
    return;
  }
  try {
    settings.value = await invoke("get_settings");
    void invoke<Diagnostics>("diagnostics").then(
      (d) => (diagnostics.value = d),
    );
    await refresh();
    unlisten = await listen<Task>("task-updated", async (event) => {
      const view = event.payload;
      const index = tasks.value.findIndex((x) => x.id === view.id);
      const old = index >= 0 ? tasks.value[index] : null;
      if (
        old &&
        old.attempt_id === view.attempt_id &&
        old.event_seq >= view.event_seq
      )
        return;
      if (index >= 0) tasks.value[index] = view;
      else tasks.value.unshift(view);
      if (complete(view) && old && !complete(old) && settings.value.notify) {
        try {
          let granted = await isPermissionGranted();
          if (!granted) granted = (await requestPermission()) === "granted";
          if (granted)
            sendNotification({
              title: "M3U8 Studio",
              body: t(
                "媒体处理已完成，打开应用查看结果。",
                "Media processing finished. Open the app for details.",
              ),
            });
        } catch {}
      }
    });
    if (localStorage.getItem("usage-notice") !== "accepted")
      modal.value = "notice";
    poll = setInterval(() => {
      void clipboardCheck();
    }, 2500);
  } catch (e) {
    pageError.value = message(e);
  }
});
onBeforeUnmount(() => {
  document.removeEventListener("keydown", dialogKeys);
  systemTheme.removeEventListener("change", applyTheme);
  unlisten?.();
  if (poll) clearInterval(poll);
  void closePlayer();
});
</script>

<template>
  <div class="app-shell" :inert="!!modal || playerVisible">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark"><ArrowDownToLine :size="22" /></div>
        <div>M3U8<span>STUDIO</span></div>
        <small>LOCAL</small>
      </div>
      <div class="workspace-label">{{ t("工作空间", "WORKSPACE") }}</div>
      <nav aria-label="Workspace">
        <button :class="{ active: filter === 'all' }" @click="filter = 'all'">
          <Layers :size="18" />{{ t("全部任务", "All tasks")
          }}<span>{{ tasks.length }}</span>
        </button>
        <button
          :class="{ active: filter === 'active' }"
          @click="filter = 'active'"
        >
          <ArrowDownToLine :size="18" />{{ t("正在下载", "In progress")
          }}<span v-if="activeCount">{{ activeCount }}</span>
        </button>
        <button
          :class="{ active: filter === 'completed' }"
          @click="filter = 'completed'"
        >
          <CheckCheck :size="18" />{{ t("已完成", "Completed")
          }}<span v-if="completeCount">{{ completeCount }}</span>
        </button>
        <button
          :class="{ active: filter === 'failed' }"
          @click="filter = 'failed'"
        >
          <AlertCircle :size="18" />{{ t("需要处理", "Needs attention")
          }}<span v-if="failedCount" class="danger-count">{{
            failedCount
          }}</span>
        </button>
      </nav>
      <div class="workspace-label library">{{ t("工具", "TOOLS") }}</div>
      <nav>
        <button @click="guard(() => localPlayback())">
          <MonitorPlay :size="18" />{{ t("本地播放", "Local player") }}</button
        ><button @click="guard(openLogs)">
          <FileText :size="18" />{{ t("运行日志", "Activity log") }}</button
        ><button @click="guard(openSettings)">
          <Settings2 :size="18" />{{ t("偏好设置", "Preferences") }}
        </button>
      </nav>
      <div class="sidebar-bottom">
        <div class="privacy-icon"><ShieldCheck :size="19" /></div>
        <div>
          <b>{{ t("只在本机处理", "Local by design") }}</b>
          <p>
            {{ t("媒体与凭证不会上传", "Your media stays on your device") }}
          </p>
        </div>
      </div>
      <div class="sidebar-version">
        <span>v0.1.0</span
        ><button
          class="icon-button"
          :title="t('切换主题', 'Toggle theme')"
          @click="settings.theme = settings.theme === 'dark' ? 'light' : 'dark'"
        >
          <Sun v-if="settings.theme === 'dark'" :size="17" /><Moon
            v-else
            :size="17"
          />
        </button>
      </div>
    </aside>

    <main class="main-content">
      <header class="topbar">
        <div class="breadcrumb">
          {{ t("工作空间", "Workspace") }}<ChevronRight :size="14" /><strong>{{
            t("下载任务", "Downloads")
          }}</strong>
        </div>
        <div class="topbar-right">
          <span class="status-dot"></span
          >{{ t("本地工作模式", "Local workspace")
          }}<button
            class="icon-button"
            :title="t('设置', 'Settings')"
            @click="guard(openSettings)"
          >
            <Settings2 :size="18" />
          </button>
        </div>
      </header>
      <section class="content">
        <div class="page-heading">
          <div>
            <div class="eyebrow">YOUR MEDIA WORKSPACE</div>
            <h1>{{ t("下载工作台", "Downloads") }}</h1>
            <p>
              {{
                t(
                  "从播放列表到本地媒体，每一步都清晰可见。",
                  "From playlist to local media. Every step in view.",
                )
              }}
            </p>
          </div>
          <button class="primary" @click="newTask">
            <Plus :size="18" />{{ t("新建任务", "New download") }}
          </button>
        </div>
        <div v-if="pageError && !modal" class="alert">
          <AlertCircle :size="18" />{{ pageError
          }}<button class="icon-button" @click="pageError = ''">
            <X :size="15" />
          </button>
        </div>
        <div class="stats-grid">
          <div class="stat">
            <span>{{ t("进行中", "IN PROGRESS") }}</span>
            <div>
              <b>{{ activeCount.toString().padStart(2, "0") }}</b>
              <div class="stat-icon blue"><ArrowDownToLine :size="20" /></div>
            </div>
            <small>{{
              t("下载、解密与媒体处理", "Download, decrypt & process")
            }}</small>
          </div>
          <div class="stat">
            <span>{{ t("已完成", "COMPLETED") }}</span>
            <div>
              <b>{{ completeCount.toString().padStart(2, "0") }}</b>
              <div class="stat-icon green"><CheckCheck :size="20" /></div>
            </div>
            <small>{{ t("可在本地随时访问", "Ready whenever you are") }}</small>
          </div>
          <div class="stat">
            <span>{{ t("当前传输速度", "TRANSFER RATE") }}</span>
            <div>
              <b class="rate">{{ formatBytes(speed) }}<em>/s</em></b>
              <div class="stat-icon purple">
                <SlidersHorizontal :size="20" />
              </div>
            </div>
            <small>{{ t("所有活动任务", "Across active tasks") }}</small>
          </div>
        </div>
        <div class="task-panel">
          <div class="panel-toolbar">
            <div class="panel-heading">
              {{ t("任务列表", "Task queue")
              }}<span>{{ visibleTasks.length }}</span>
            </div>
            <div class="toolbar-actions">
              <label class="search"
                ><Search :size="16" /><input
                  v-model="query"
                  :placeholder="t('搜索任务…', 'Search tasks…')" /></label
              ><button
                class="icon-button"
                :title="t('刷新', 'Refresh')"
                @click="guard(refresh)"
              >
                <RefreshCw :size="16" /></button
              ><button
                class="secondary compact"
                @click="
                  newTask();
                  batchMode = true;
                "
              >
                <Layers :size="15" />{{ t("批量导入", "Batch import") }}
              </button>
            </div>
          </div>
          <div v-if="selected.length" class="selection-bar">
            <span>{{ t("已选择", "Selected") }} {{ selected.length }}</span
            ><button @click="batchAction('start')">
              <Play :size="14" />{{ t("开始", "Start") }}</button
            ><button @click="batchAction('pause')">
              <Pause :size="14" />{{ t("暂停", "Pause") }}</button
            ><button @click="batchAction('cancel')">
              <Square :size="14" />{{ t("取消", "Cancel") }}</button
            ><button @click="selected = []">
              {{ t("取消选择", "Clear selection") }}
            </button>
          </div>
          <div class="table-head">
            <input
              type="checkbox"
              :checked="
                !!visibleTasks.length && selected.length === visibleTasks.length
              "
              aria-label="Select all"
              @change="selectAll"
            /><span>{{ t("媒体 / 来源", "MEDIA / SOURCE") }}</span
            ><span>{{ t("状态与进度", "STATUS & PROGRESS") }}</span
            ><span>{{ t("大小", "SIZE") }}</span
            ><span>{{ t("操作", "ACTIONS") }}</span>
          </div>
          <div v-if="!visibleTasks.length" class="empty-state">
            <div class="empty-illustration">
              <div class="back-card"></div>
              <div class="front-card">
                <Film :size="31" />
                <div class="mini-line"></div>
                <div class="mini-line short"></div>
                <div class="download-badge"><ArrowDownToLine :size="20" /></div>
              </div>
            </div>
            <h2>
              {{
                query
                  ? t("没有匹配的任务", "No matching tasks")
                  : t(
                      "准备好保存你的第一段媒体",
                      "Ready for your first download",
                    )
              }}
            </h2>
            <p>
              {{
                t(
                  "添加一个 M3U8 链接，选择轨道，即可开始。",
                  "Add an M3U8 link, choose your tracks, and get started.",
                )
              }}
            </p>
            <button class="secondary" @click="newTask">
              <Link :size="16" />{{ t("添加播放列表", "Add a playlist") }}
            </button>
            <div class="format-chips">
              <span>HLS / M3U8</span><span>AES-128</span
              ><span>MP4 · MKV · TS</span>
            </div>
          </div>
          <div
            v-for="task in visibleTasks"
            :key="task.id"
            class="task-row"
            :class="{ focused: selectedTask === task.id }"
            @click="selectedTask = task.id"
          >
            <input
              type="checkbox"
              :checked="selected.includes(task.id)"
              :aria-label="`Select ${task.title}`"
              @click.stop
              @change="toggleSelected(task.id)"
            />
            <div class="task-identity">
              <div class="file-icon"><Film :size="20" /></div>
              <div>
                <button class="task-title" @click="selectedTask = task.id">
                  {{ task.title }}
                </button>
                <p>
                  {{ task.source_display
                  }}<span v-if="task.resolution !== 'unknown'">
                    · {{ task.resolution }}</span
                  >
                </p>
              </div>
            </div>
            <div class="task-progress">
              <div>
                <span class="state" :class="task.state"
                  ><LoaderCircle
                    v-if="running(task)"
                    :size="12"
                    class="spin"
                  /><Check
                    v-else-if="task.state === 'completed'"
                    :size="12"
                  /><AlertCircle
                    v-else-if="
                      ['failed', 'waiting', 'completed_with_gaps'].includes(
                        task.state,
                      )
                    "
                    :size="12"
                  />{{ stateLabel(task.state) }}</span
                ><small>{{
                  task.state === "downloading"
                    ? formatBytes(task.speed) + "/s"
                    : task.total
                      ? `${task.done}/${task.total}`
                      : ""
                }}</small>
              </div>
              <div class="progress-track">
                <div
                  :style="{
                    width: `${Math.max(0, Math.min(100, task.progress * 100))}%`,
                  }"
                  :class="task.state"
                ></div>
              </div>
              <small class="stage-text" :title="task.stage">{{
                en ? stateLabel(task.state) : task.stage
              }}</small>
            </div>
            <div class="task-size">
              {{ task.bytes ? formatBytes(task.bytes) : "—" }}
            </div>
            <div class="row-actions" @click.stop>
              <button
                v-if="running(task)"
                class="icon-button"
                :title="t('暂停', 'Pause')"
                @click="action(task, 'pause')"
              >
                <Pause :size="16" /></button
              ><button
                v-else-if="!complete(task)"
                class="icon-button"
                :title="t('开始 / 重试', 'Start / retry')"
                @click="action(task, 'start')"
              >
                <Play :size="16" /></button
              ><button
                v-if="task.output"
                class="icon-button"
                :title="t('打开文件夹', 'Reveal output')"
                @click="guard(() => revealItemInDir(task.output!))"
              >
                <FolderOpen :size="16" /></button
              ><button
                class="icon-button"
                :title="t('预览', 'Preview')"
                @click="guard(() => playTask(task))"
              >
                <MonitorPlay :size="16" /></button
              ><button
                class="icon-button destructive"
                :title="t('删除', 'Delete')"
                @click="remove(task)"
              >
                <Trash2 :size="16" />
              </button>
            </div>
          </div>
          <div class="panel-footer">
            <span
              ><ShieldCheck :size="13" />{{
                t(
                  "凭证按来源隔离 · 输出前验证媒体",
                  "Scoped credentials · Verified media output",
                )
              }}</span
            ><span>{{ tasks.length }} {{ t("个任务", "tasks") }}</span>
          </div>
        </div>
        <div v-if="focusTask" class="detail-card">
          <div>
            <span class="eyebrow">TASK DETAILS</span>
            <h3>{{ focusTask.title }}</h3>
            <p class="mono">{{ focusTask.id }}</p>
          </div>
          <div class="detail-actions">
            <button
              class="secondary compact"
              @click="guard(() => edit(focusTask!))"
            >
              <Settings2 :size="14" />{{ t("编辑配置", "Edit") }}</button
            ><button
              class="secondary compact"
              @click="guard(() => edit(focusTask!, true))"
            >
              <Copy :size="14" />{{ t("复制任务", "Duplicate") }}</button
            ><button class="icon-button" @click="selectedTask = null">
              <X :size="16" />
            </button>
          </div>
          <div v-if="focusTask.error" class="detail-error">
            <b>{{ focusTask.error.code }}</b
            >{{ focusTask.error.message }}
          </div>
          <p v-if="focusTask.output" class="output-path">
            {{ focusTask.output }}
          </p>
          <button
            v-if="focusTask.error?.code === 'MISSING_CONFIRMATION'"
            class="primary"
            @click="action(focusTask, 'confirm_gaps')"
          >
            {{
              t(
                "确认下列缺失并生成不完整输出",
                "Confirm gaps and create incomplete output",
              )
            }}
          </button>
          <p v-if="focusTask.missing.length" class="detail-error">
            {{ t("缺失分片", "Missing segments") }}:
            {{ focusTask.missing.join(", ") }}
          </p>
          <div v-if="details?.total" class="segment-details">
            <div class="parsed-actions">
              <b
                >{{ t("分片与密钥上下文", "Segments & key contexts") }} ·
                {{ details.total }}</b
              ><button class="text-button" @click="guard(loadDetails)">
                {{ t("刷新", "Refresh") }}
              </button>
            </div>
            <div class="table-scroll">
              <table class="segment-table">
                <thead>
                  <tr>
                    <th>ID</th>
                    <th>{{ t("序号 / 时长", "Sequence / duration") }}</th>
                    <th>{{ t("状态 / 大小", "State / size") }}</th>
                    <th>{{ t("AES-128 / IV", "AES-128 / IV") }}</th>
                    <th>{{ t("来源", "Source") }}</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="seg in details.segments" :key="seg.resource">
                    <td>{{ seg.resource }} · {{ seg.track }}</td>
                    <td>{{ seg.sequence }} / {{ seg.duration.toFixed(3) }}s</td>
                    <td>{{ seg.state }} / {{ formatBytes(seg.bytes) }}</td>
                    <td>{{ seg.context || "—" }} / {{ seg.iv || "—" }}</td>
                    <td>{{ seg.source }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <div class="parsed-actions">
              <button
                class="text-button"
                :disabled="detailOffset === 0"
                @click="
                  detailOffset -= 20;
                  guard(loadDetails);
                "
              >
                {{ t("上一页", "Previous") }}</button
              ><span
                >{{ detailOffset + 1 }}–{{
                  Math.min(detailOffset + 20, details.total)
                }}</span
              ><button
                class="text-button"
                :disabled="detailOffset + 20 >= details.total"
                @click="
                  detailOffset += 20;
                  guard(loadDetails);
                "
              >
                {{ t("下一页", "Next") }}
              </button>
            </div>
            <details>
              <summary>{{ t("最近执行记录", "Recent attempts") }}</summary>
              <p
                v-for="attempt in details.attempts"
                :key="attempt.attempt_id"
                class="mono"
              >
                {{ attempt.attempt_id.slice(0, 8) }} ·
                {{ stateLabel(attempt.state) }} ·
                {{ attempt.error?.code || "—" }}
              </p>
            </details>
          </div>
          <div class="detail-bottom">
            <span>{{ new Date(focusTask.created_at).toLocaleString() }}</span
            ><button
              class="text-button"
              @click="action(focusTask, 'clear_cache')"
            >
              {{ t("清理分片缓存", "Clear segment cache") }}
            </button>
          </div>
        </div>
        <div class="workspace-note">
          <HardDrive :size="15" /><span
            >{{ t("保存位置", "Save to") }}
            <b>{{
              settings.output_dir ||
              t("在设置中选择输出目录", "Choose a destination in settings")
            }}</b></span
          ><button @click="guard(openSettings)">
            {{ t("更改", "Change") }}<ArrowUpRight :size="13" />
          </button>
        </div>
      </section>
    </main>
  </div>

  <div
    v-if="modal"
    class="modal-backdrop"
    @mousedown.self="modal !== 'notice' && (modal = '')"
  >
    <section
      v-if="modal === 'new'"
      class="dialog large"
      role="dialog"
      aria-modal="true"
      :aria-label="t('新建下载任务', 'New download')"
    >
      <header class="dialog-header">
        <div>
          <span class="eyebrow">NEW DOWNLOAD</span>
          <h2>
            {{
              editing
                ? t("编辑任务", "Edit task")
                : t("添加下载任务", "Add a download")
            }}
          </h2>
        </div>
        <button class="icon-button" @click="modal = ''">
          <X :size="20" />
        </button>
      </header>
      <div class="dialog-body">
        <div class="segmented">
          <button :class="{ active: !batchMode }" @click="batchMode = false">
            {{ t("单个链接", "Single source") }}</button
          ><button
            :disabled="!!editing"
            :class="{ active: batchMode }"
            @click="batchMode = true"
          >
            {{ t("批量导入", "Batch import") }}
          </button>
        </div>
        <label v-if="batchMode"
          >{{ t("播放列表链接，每行一条", "Playlist URLs, one per line")
          }}<textarea
            v-model="batch"
            rows="5"
            placeholder="https://example.com/video/index.m3u8"
          ></textarea>
        </label>
        <label v-else
          >{{ t("播放列表地址或本地路径", "Playlist URL or local path") }}
          <div class="input-action">
            <input
              v-model="input.source"
              placeholder="https://example.com/video/index.m3u8"
              @input="parsed = null"
            /><button
              class="icon-button"
              :title="t('选择本地文件', 'Choose local playlist')"
              @click="guard(chooseSource)"
            >
              <FolderOpen :size="17" /></button
            ><button
              class="secondary compact"
              :disabled="busy || !input.source"
              @click="inspect"
            >
              <LoaderCircle v-if="busy" class="spin" :size="15" />{{
                t("解析", "Analyze")
              }}
            </button>
          </div></label
        >
        <div v-if="pageError" class="alert">
          <AlertCircle :size="17" />{{ pageError }}
        </div>
        <div v-if="parsed" class="parsed-card">
          <div>
            <ShieldCheck :size="18" /><b>{{
              parsed.master
                ? t("多清晰度播放列表", "Master playlist")
                : `${parsed.segments.length} ${t("个分片", "segments")}`
            }}</b
            ><span>{{
              parsed.master
                ? `${parsed.variants.length} ${t("种清晰度", "variants")}`
                : parsed.ended
                  ? t("点播资源", "VOD")
                  : t("尚未结束，不支持录制", "Live playlist is unsupported")
            }}</span>
          </div>
          <div v-if="parsed.master" class="form-grid">
            <label
              >{{ t("清晰度", "Variant")
              }}<select v-model="input.variant">
                <option v-for="(v, i) in parsed.variants" :key="i" :value="i">
                  {{ v.resolution || "Unknown" }} ·
                  {{ (v.bandwidth / 1000).toFixed(0) }} kbps
                </option>
              </select></label
            ><label
              >{{ t("音轨", "Audio")
              }}<select v-model="input.audio">
                <option :value="null">
                  {{ t("默认音轨", "Default audio") }}
                </option>
                <option
                  v-for="(r, i) in parsed.renditions"
                  v-show="r.kind === 'AUDIO'"
                  :key="i"
                  :value="i"
                >
                  {{ r.name }} {{ r.language }}
                </option>
              </select></label
            ><label
              >{{ t("字幕", "Subtitles")
              }}<select v-model="input.subtitle">
                <option :value="null">
                  {{ t("不下载字幕", "No subtitles") }}
                </option>
                <option
                  v-for="(r, i) in parsed.renditions"
                  v-show="r.kind === 'SUBTITLES'"
                  :key="i"
                  :value="i"
                >
                  {{ r.name }}
                </option>
              </select></label
            >
          </div>
          <div class="parsed-actions">
            <button class="text-button" @click="preview()">
              <MonitorPlay :size="15" />{{
                t("在线预览", "Preview stream")
              }}</button
            ><button class="text-button" @click="showSource = !showSource">
              {{
                showSource
                  ? t("隐藏原文", "Hide source")
                  : t(
                      "查看原文（可能含凭证）",
                      "Show source (may contain secrets)",
                    )
              }}
            </button>
          </div>
          <div v-if="!parsed.master" class="table-scroll">
            <table class="segment-table">
              <thead>
                <tr>
                  <th>{{ t("媒体序号", "Sequence") }}</th>
                  <th>{{ t("时长", "Duration") }}</th>
                  <th>{{ t("密钥上下文", "Key context") }}</th>
                  <th>IV</th>
                  <th>{{ t("来源（脱敏）", "Source (redacted)") }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="seg in parsed.segments.slice(0, 50)"
                  :key="seg.index"
                >
                  <td>{{ seg.sequence }}</td>
                  <td>{{ seg.duration.toFixed(3) }}s</td>
                  <td>{{ seg.key ? `0:${seg.key.id}` : "—" }}</td>
                  <td>
                    {{ seg.key?.iv || t("媒体序号推导", "Media sequence") }}
                  </td>
                  <td>{{ safeUri(seg.uri) }}</td>
                </tr>
              </tbody>
            </table>
            <p v-if="parsed.segments.length > 50" class="form-hint">
              {{
                t(
                  "显示前 50 项；创建任务后可在详情分页浏览全部分片。",
                  "First 50 shown. All segments are available in task details.",
                )
              }}
            </p>
          </div>
          <p
            v-for="warning in parsed.warnings"
            :key="warning"
            class="form-hint"
          >
            {{ warning }}
          </p>
          <pre v-if="showSource" class="source-code">{{ parsed.raw }}</pre>
        </div>
        <div class="form-grid two">
          <label
            >{{ t("任务名称", "Task name")
            }}<input
              v-model="input.title"
              :placeholder="
                t('可选，留空自动生成', 'Optional, generated if empty')
              " /></label
          ><label
            >{{ t("保存目录", "Destination") }}
            <div class="input-action">
              <input v-model="input.output_dir" /><button
                class="icon-button"
                @click="guard(() => chooseDirectory('input'))"
              >
                <FolderOpen :size="17" />
              </button></div
          ></label>
        </div>
        <div class="form-grid">
          <label
            >{{ t("输出容器", "Container")
            }}<select v-model="input.container">
              <option value="mp4">MP4</option>
              <option value="mkv">MKV</option>
              <option value="ts">TS</option>
            </select></label
          ><label
            >{{ t("视频编码", "Video codec")
            }}<select v-model="input.video_codec">
              <option value="copy">
                {{ t("保留原编码 · copy", "Keep original · copy") }}
              </option>
              <option
                value="h264"
                :disabled="
                  !!diagnostics?.capabilities && !diagnostics.capabilities.h264
                "
              >
                H.264
              </option>
              <option
                value="h265"
                :disabled="
                  !!diagnostics?.capabilities && !diagnostics.capabilities.h265
                "
              >
                H.265
              </option>
            </select></label
          ><label
            >{{ t("音频编码", "Audio codec")
            }}<select v-model="input.audio_codec">
              <option value="copy">
                {{ t("保留原编码 · copy", "Keep original · copy") }}
              </option>
              <option
                value="aac"
                :disabled="
                  !!diagnostics?.capabilities && !diagnostics.capabilities.aac
                "
              >
                AAC
              </option>
            </select></label
          >
        </div>
        <details class="advanced">
          <summary>
            <SlidersHorizontal :size="16" />{{
              t("请求、代理与解密", "Requests, proxy & decryption")
            }}
          </summary>
          <div class="form-grid two">
            <label>User-Agent<input v-model="input.network.user_agent" /></label
            ><label
              >{{
                t(
                  "代理（direct / system / URL）",
                  "Proxy (direct / system / URL)",
                )
              }}<input
                v-model="input.network.proxy"
                placeholder="socks5h://127.0.0.1:1080" /></label
            ><label
              >{{
                t(
                  "请求头 JSON（Cookie、Referer 等）",
                  "Headers JSON (Cookie, Referer, etc.)",
                )
              }}<textarea
                v-model="headerText"
                rows="4"
                spellcheck="false"
              ></textarea></label
            ><label
              >{{ t("其他源的请求头 JSON", "Headers for other origins JSON")
              }}<textarea
                v-model="originHeaderText"
                rows="4"
                spellcheck="false"
                placeholder='{"https://cdn.example.com":{"Cookie":"..."}}'
              ></textarea></label
            ><label
              >{{
                t(
                  "手动密钥 JSON（上下文编号 → 密钥）",
                  "Manual keys JSON (context ID → key)",
                )
              }}<textarea
                v-model="keysText"
                rows="3"
                spellcheck="false"
              ></textarea></label
            ><label
              >{{
                t(
                  "手动 IV JSON（上下文编号 → IV）",
                  "Manual IVs JSON (context ID → IV)",
                )
              }}<textarea
                v-model="ivsText"
                rows="3"
                spellcheck="false"
              ></textarea></label
            ><label
              >{{
                t(
                  "未标记 AES-128 的手动密钥（可选）",
                  "AES-128 key for unmarked source (optional)",
                )
              }}<input v-model="input.force_key" type="password" /><span
                v-if="input.force_key"
                class="form-grid"
                ><label
                  >{{ t("轨道编号（视频 0）", "Track index (video 0)")
                  }}<input
                    v-model.number="input.force_track"
                    type="number"
                    min="0" /></label
                ><label
                  >{{ t("开始媒体序号", "First sequence")
                  }}<input
                    v-model.number="forceStart"
                    type="number"
                    min="0" /></label
                ><label
                  >{{ t("结束媒体序号", "Last sequence")
                  }}<input
                    v-model.number="forceEnd"
                    type="number"
                    min="0" /></label></span></label
            ><label
              >{{ t("并发请求数", "Concurrent requests")
              }}<input
                v-model.number="input.network.concurrency"
                type="number"
                min="1"
                max="32" /></label
            ><label
              >{{ t("限速 KiB/s（0 不限速）", "Rate KiB/s (0 unlimited)")
              }}<input
                v-model.number="input.network.limit_kib"
                type="number"
                min="0" /></label
            ><label
              >{{ t("重试次数", "Retries")
              }}<input
                v-model.number="input.network.retries"
                type="number"
                min="0"
                max="10" /></label
            ><label
              >{{ t("本地资源根目录（可选）", "Local resource root (optional)")
              }}<input v-model="input.local_root" /></label
            ><label
              >{{
                t(
                  "本地清单网络基础地址（可选）",
                  "Network base for local manifest (optional)",
                )
              }}<input v-model="input.base_url"
            /></label>
          </div>
          <button
            class="secondary compact"
            :disabled="busy || !input.source"
            @click="connectionTest"
          >
            {{ t("测试当前来源连接", "Test connection to source") }}
          </button>
          <p class="form-hint">
            {{
              t(
                "socks5h 使用代理端 DNS；socks5 使用本机 DNS。代理不可用时不会直连。",
                "socks5h resolves DNS on the proxy; socks5 resolves locally. There is no direct fallback.",
              )
            }}
          </p>
          <label class="checkbox-label"
            ><input v-model="input.remember" type="checkbox" />{{
              t(
                "使用操作系统安全存储记住此任务凭证",
                "Remember credentials using OS secure storage",
              )
            }}</label
          >
          <p class="form-hint">
            {{
              t(
                "默认仅向输入来源发送请求头；跨源资源请单独配置。",
                "Headers are scoped to the input origin; configure other origins explicitly.",
              )
            }}
          </p>
        </details>
        <details class="advanced">
          <summary>
            <Settings2 :size="16" />{{
              t("输出与缓存选项", "Output & cache options")
            }}
          </summary>
          <label
            >{{ t("文件名模板", "Filename template")
            }}<input v-model="input.filename_template"
          /></label>
          <div class="form-grid two">
            <label
              >{{ t("字幕输出", "Subtitle output")
              }}<select v-model="input.subtitle_mode">
                <option value="sidecar">
                  {{ t("独立 VTT 文件", "Separate VTT file") }}
                </option>
                <option value="embed">
                  {{ t("封装软字幕", "Embed soft subtitles") }}
                </option>
              </select></label
            ><label class="checkbox-label"
              ><input v-model="input.video_only" type="checkbox" />{{
                t("仅视频，不下载音轨", "Video only")
              }}</label
            >
          </div>
          <label class="checkbox-label"
            ><input v-model="input.overwrite" type="checkbox" />{{
              t(
                "允许覆盖同名输出（新结果验证成功后替换）",
                "Replace existing output after successful validation",
              )
            }}</label
          >
          <label class="checkbox-label"
            ><input v-model="input.keep_cache" type="checkbox" />{{
              t("成功后保留分片缓存", "Keep segment cache after success")
            }}</label
          ><label class="checkbox-label"
            ><input v-model="input.fast_ts" type="checkbox" />{{
              t(
                "快速 TS 拼接（仅适用兼容输入）",
                "Fast TS concatenation (compatible inputs only)",
              )
            }}</label
          ><label class="checkbox-label"
            ><input v-model="input.allow_gaps" type="checkbox" />{{
              t(
                "允许跳过失败媒体分片，输出标为不完整",
                "Allow missing media segments; mark output incomplete",
              )
            }}</label
          >
        </details>
      </div>
      <footer class="dialog-footer">
        <span
          ><ShieldCheck :size="14" />{{
            t(
              "仅处理你有权使用的媒体",
              "Only process media you are authorized to use",
            )
          }}</span
        ><button class="secondary" :disabled="busy" @click="submit(false)">
          {{ t("加入队列", "Save task") }}</button
        ><button
          class="primary"
          :disabled="busy || (!batchMode && !input.source)"
          @click="submit(true)"
        >
          <LoaderCircle v-if="busy" class="spin" :size="16" /><ArrowDownToLine
            v-else
            :size="16"
          />{{ t("开始下载", "Start download") }}
        </button>
      </footer>
    </section>

    <section
      v-else-if="modal === 'settings'"
      class="dialog large"
      role="dialog"
      aria-modal="true"
    >
      <header class="dialog-header">
        <div>
          <span class="eyebrow">PREFERENCES</span>
          <h2>{{ t("偏好设置", "Preferences") }}</h2>
        </div>
        <button class="icon-button" @click="modal = ''">
          <X :size="20" />
        </button>
      </header>
      <div class="dialog-body">
        <div class="section-label">
          {{ t("文件与存储", "FILES & STORAGE") }}
        </div>
        <label
          >{{ t("默认输出目录", "Default destination") }}
          <div class="input-action">
            <input v-model="settingsDraft.output_dir" /><button
              class="icon-button"
              @click="guard(() => chooseDirectory('output'))"
            >
              <FolderOpen :size="17" />
            </button></div></label
        ><label
          >{{ t("分片缓存目录", "Segment cache") }}
          <div class="input-action">
            <input v-model="settingsDraft.cache_dir" /><button
              class="icon-button"
              @click="guard(() => chooseDirectory('cache'))"
            >
              <FolderOpen :size="17" />
            </button></div
        ></label>
        <div class="form-grid two">
          <label
            >{{ t("低磁盘阈值 MiB", "Low disk threshold MiB")
            }}<input
              v-model.number="settingsDraft.low_disk_mib"
              type="number"
              min="0" /></label
          ><label class="checkbox-label"
            ><input v-model="settingsDraft.keep_cache" type="checkbox" />{{
              t("默认保留完成任务缓存", "Keep completed caches by default")
            }}</label
          >
        </div>
        <div class="section-label">
          {{ t("下载与网络", "DOWNLOAD & NETWORK") }}
        </div>
        <div class="form-grid">
          <label
            >{{ t("同时下载任务", "Active tasks")
            }}<input
              v-model.number="settingsDraft.max_tasks"
              type="number"
              min="1"
              max="8" /></label
          ><label
            >{{ t("全局请求数", "Global requests")
            }}<input
              v-model.number="settingsDraft.max_requests"
              type="number"
              min="1"
              max="64" /></label
          ><label
            >{{ t("每任务请求数", "Requests per task")
            }}<input
              v-model.number="settingsDraft.network.concurrency"
              type="number"
              min="1"
              max="32" /></label
          ><label
            >{{ t("后处理并发", "Processing jobs")
            }}<input
              v-model.number="settingsDraft.max_processing"
              type="number"
              min="1"
              max="4" /></label
          ><label
            >{{ t("连接超时（秒）", "Connect timeout (s)")
            }}<input
              v-model.number="settingsDraft.network.connect_timeout"
              type="number"
              min="1"
              max="120" /></label
          ><label
            >{{ t("读取超时（秒）", "Read timeout (s)")
            }}<input
              v-model.number="settingsDraft.network.read_timeout"
              type="number"
              min="5"
              max="300"
          /></label>
        </div>
        <label
          >{{ t("默认代理", "Default proxy")
          }}<input
            v-model="settingsDraft.network.proxy"
            placeholder="direct / system / http://... / socks5h://..." /></label
        ><label
          >{{
            t(
              "按源绑定的默认请求头 JSON（默认仅本次运行）",
              "Default headers by origin (session only by default)",
            )
          }}<textarea
            v-model="defaultHeaders"
            rows="3"
            placeholder='{"https://example.com":{"Cookie":"..."}}'
          ></textarea>
        </label>
        <div class="form-grid">
          <label
            >{{ t("默认 User-Agent", "Default User-Agent")
            }}<input v-model="settingsDraft.network.user_agent" /></label
          ><label
            >{{ t("请求总时限（秒）", "Request timeout (s)")
            }}<input
              v-model.number="settingsDraft.network.request_timeout"
              type="number"
              min="1"
              max="3600" /></label
          ><label
            >{{ t("最大重定向", "Redirect limit")
            }}<input
              v-model.number="settingsDraft.network.max_redirects"
              type="number"
              min="0"
              max="10" /></label
          ><label
            >{{ t("历史保留天数（0 手动）", "History days (0 manual)")
            }}<input
              v-model.number="settingsDraft.history_days"
              type="number"
              min="0" /></label
          ><label
            >{{ t("日志级别", "Log level")
            }}<select v-model="settingsDraft.log_level">
              <option>DEBUG</option>
              <option>INFO</option>
              <option>WARN</option>
              <option>ERROR</option>
            </select></label
          >
        </div>
        <button class="secondary compact" @click="guard(clearHistory)">
          {{ t("清理已完成与已取消记录", "Clear completed/cancelled history") }}
        </button>
        <div class="section-label">
          {{ t("界面与提醒", "APPEARANCE & NOTIFICATIONS") }}
        </div>
        <div class="form-grid two">
          <label
            >{{ t("主题", "Theme")
            }}<select v-model="settingsDraft.theme">
              <option value="system">{{ t("跟随系统", "System") }}</option>
              <option value="light">{{ t("浅色", "Light") }}</option>
              <option value="dark">{{ t("深色", "Dark") }}</option>
            </select></label
          ><label
            >{{ t("语言", "Language")
            }}<select v-model="settingsDraft.language">
              <option value="zh-CN">简体中文</option>
              <option value="en">English</option>
            </select></label
          >
        </div>
        <label class="checkbox-label"
          ><input v-model="settingsDraft.notify" type="checkbox" />{{
            t("任务完成时发送桌面通知", "Notify when a task finishes")
          }}</label
        ><label class="checkbox-label"
          ><input v-model="clipboard" type="checkbox" /><Clipboard
            :size="14"
          />{{
            t(
              "本次运行监听剪贴板链接（确认后才使用）",
              "Watch clipboard this session (confirm before use)",
            )
          }}</label
        >
        <div class="section-label">{{ t("媒体处理依赖", "MEDIA TOOLS") }}</div>
        <label>FFmpeg<input v-model="settingsDraft.ffmpeg" /></label
        ><label>ffprobe<input v-model="settingsDraft.ffprobe" /></label>
        <div class="dependency-card">
          <ShieldCheck :size="17" />
          <div>
            <b>{{
              diagnostics?.secure_storage
                ? t("系统安全存储可用", "Secure storage available")
                : t(
                    "安全存储不可用，凭证仅保留在内存",
                    "Secure storage unavailable; secrets stay in memory",
                  )
            }}</b>
            <p v-if="diagnostics?.capabilities">
              FFmpeg ✓ · H.264 {{ diagnostics.capabilities.h264 ? "✓" : "—" }} ·
              H.265 {{ diagnostics.capabilities.h265 ? "✓" : "—" }} · AAC
              {{ diagnostics.capabilities.aac ? "✓" : "—" }}
            </p>
            <p v-else>
              {{
                t(
                  "未检测到可用 FFmpeg，请配置可执行文件路径。",
                  "FFmpeg unavailable. Configure its executable path.",
                )
              }}
            </p>
            <p class="mono">{{ diagnostics?.data_dir }}</p>
          </div>
        </div>
        <p class="form-hint">
          {{
            t(
              "更改设置前请暂停活动任务。新默认值不追溯修改已有任务。",
              "Pause active tasks before saving. Defaults apply to new tasks.",
            )
          }}
        </p>
      </div>
      <footer class="dialog-footer">
        <button
          class="secondary"
          @click="
            settingsDraft = {
              ...settingsDraft,
              network: networkDefaults(),
              theme: 'system',
              language: 'zh-CN',
              max_tasks: 2,
              max_requests: 16,
              max_processing: 1,
              low_disk_mib: 1024,
            }
          "
        >
          {{ t("恢复默认参数", "Reset parameters") }}</button
        ><button class="primary" :disabled="busy" @click="saveSettings">
          <Check :size="16" />{{ t("保存设置", "Save preferences") }}
        </button>
      </footer>
    </section>

    <section
      v-else-if="modal === 'logs'"
      class="dialog large"
      role="dialog"
      aria-modal="true"
    >
      <header class="dialog-header">
        <div>
          <span class="eyebrow">ACTIVITY</span>
          <h2>{{ t("运行日志", "Activity log") }}</h2>
        </div>
        <button class="icon-button" @click="modal = ''">
          <X :size="20" />
        </button>
      </header>
      <div class="dialog-body">
        <label
          >{{
            t(
              "按任务 ID、状态或错误码筛选",
              "Filter by task ID, state or error code",
            )
          }}<input v-model="logQuery"
        /></label>
        <pre class="log-output">{{
          logs
            .split("\n")
            .filter((l) => l.toLowerCase().includes(logQuery.toLowerCase()))
            .join("\n") || t("暂无日志", "No log entries")
        }}</pre>
        <p class="form-hint">
          {{
            t(
              "日志不包含密钥、Cookie 或完整签名链接。",
              "Logs exclude keys, cookies and signed URLs.",
            )
          }}
        </p>
      </div>
      <footer class="dialog-footer">
        <button class="secondary" @click="guard(openLogs)">
          <RefreshCw :size="15" />{{ t("刷新", "Refresh") }}</button
        ><button class="primary" @click="exportLogs">
          <Download :size="16" />{{ t("导出脱敏日志", "Export sanitized log") }}
        </button>
      </footer>
    </section>

    <section
      v-else-if="modal === 'notice'"
      class="dialog notice"
      role="dialog"
      aria-modal="true"
    >
      <div class="notice-icon"><ShieldCheck :size="32" /></div>
      <h2>{{ t("欢迎使用 M3U8 Studio", "Welcome to M3U8 Studio") }}</h2>
      <p>
        {{
          t(
            "这是一个在本机运行的媒体处理工具。请仅下载或处理你拥有版权、已获授权，或依法可以使用的资源。",
            "A local media workspace. Only download or process resources you own or are authorized to use.",
          )
        }}
      </p>
      <p>
        {{
          t(
            "退出应用时，活动任务会暂停并保存恢复信息。未选择记住的凭证不会跨重启保存。",
            "Active tasks pause on exit. Credentials are not retained unless you choose to remember them.",
          )
        }}
      </p>
      <label class="checkbox-label"
        ><input v-model="acceptNotice" type="checkbox" />{{
          t(
            "我已了解并同意合理使用本工具",
            "I understand and will use this tool responsibly",
          )
        }}</label
      ><button class="primary" :disabled="!acceptNotice" @click="acceptUsage">
        {{ t("进入工作台", "Open workspace") }}<ChevronRight :size="16" />
      </button>
    </section>
  </div>

  <div v-if="playerVisible" class="modal-backdrop player-backdrop">
    <section class="dialog player-dialog" role="dialog" aria-modal="true">
      <header class="dialog-header">
        <div>
          <span class="eyebrow">PREVIEW</span>
          <h2>{{ playerTitle }}</h2>
        </div>
        <button class="icon-button" @click="closePlayer">
          <X :size="20" />
        </button>
      </header>
      <video ref="video" controls playsinline>
        <track
          v-if="subtitleUrl"
          :src="subtitleUrl"
          kind="subtitles"
          label="Subtitles"
          default
        />
      </video>
      <div v-if="playerError" class="alert">
        <AlertCircle :size="17" />{{ playerError }}
      </div>
      <footer class="dialog-footer">
        <span>{{
          t(
            "预览流量计入统一请求限制",
            "Preview uses the shared request policy",
          )
        }}</span
        ><button class="secondary" @click="guard(attachSubtitle)">
          {{ t("挂载 VTT 字幕", "Attach VTT") }}
        </button>
      </footer>
    </section>
  </div>
  <div v-if="toast" class="toast" role="status">
    <AlertCircle :size="17" /><span>{{ toast }}</span
    ><button class="icon-button" @click="toast = ''"><X :size="15" /></button>
  </div>
</template>
