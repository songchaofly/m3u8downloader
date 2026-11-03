import Hls, {
  type LoaderContext,
  type LoaderConfiguration,
  type LoaderCallbacks,
  type Loader,
} from "hls.js";
import { invoke } from "@tauri-apps/api/core";

export function createPlayer(
  video: HTMLVideoElement,
  id: string,
  manifest: string,
  onError: (message: string) => void,
): Hls {
  const root = `https://preview.invalid/${id}/master`;
  class NativeLoader implements Loader<LoaderContext> {
    context: LoaderContext | null = null;
    stats = {
      aborted: false,
      loaded: 0,
      retry: 0,
      total: 0,
      chunkCount: 0,
      bwEstimate: 0,
      loading: { start: 0, first: 0, end: 0 },
      parsing: { start: 0, end: 0 },
      buffering: { start: 0, first: 0, end: 0 },
    };
    aborted = false;
    constructor(_config: unknown) {}
    destroy() {
      this.aborted = true;
    }
    abort() {
      this.aborted = true;
      this.stats.aborted = true;
    }
    load(
      context: LoaderContext,
      _config: LoaderConfiguration,
      callbacks: LoaderCallbacks<LoaderContext>,
    ) {
      this.context = context;
      this.stats.loading.start = performance.now();
      const u = new URL(context.url);
      if (
        u.origin !== "https://preview.invalid" ||
        !u.pathname.startsWith(`/${id}/`)
      ) {
        callbacks.onError(
          { code: 403, text: "Unknown preview resource" },
          context,
          null,
          this.stats,
        );
        return;
      }
      const token = u.pathname.split("/").at(-1)!;
      const promise =
        context.url === root
          ? Promise.resolve(new TextEncoder().encode(manifest).buffer)
          : invoke<ArrayBuffer>("preview_resource", { id, token });
      promise
        .then((buffer) => {
          if (this.aborted) return;
          const bytes =
            buffer instanceof ArrayBuffer
              ? buffer
              : new Uint8Array(buffer as unknown as number[]).buffer;
          this.stats.loading.first = this.stats.loading.end = performance.now();
          this.stats.loaded = this.stats.total = bytes.byteLength;
          const data =
            context.responseType === "arraybuffer"
              ? bytes
              : new TextDecoder().decode(bytes);
          callbacks.onSuccess(
            { url: context.url, data },
            this.stats,
            context,
            null,
          );
        })
        .catch((error) => {
          if (!this.aborted)
            callbacks.onError(
              { code: 0, text: error?.message || "资源加载失败" },
              context,
              null,
              this.stats,
            );
        });
    }
  }
  const hls = new Hls({
    loader: NativeLoader,
    maxBufferLength: 20,
    maxMaxBufferLength: 30,
    maxBufferSize: 64 * 1024 * 1024,
    enableWorker: false,
  });
  hls.on(Hls.Events.ERROR, (_event, data) => {
    if (data.fatal) onError(data.error?.message || data.details);
  });
  hls.loadSource(root);
  hls.attachMedia(video);
  hls.on(Hls.Events.MANIFEST_PARSED, () => {
    void video.play().catch(() => {});
  });
  return hls;
}
