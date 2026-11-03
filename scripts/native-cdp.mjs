// Minimal measurement driver: no Playwright injected scripts, tracing or enabled domains.
// A loopback debugging endpoint is still enabled only by the test process environment.
export async function connectPage(port) {
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find((target) => target.type === "page");
  if (!target?.webSocketDebuggerUrl) throw new Error("No WebView page yet");
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve, { once: true });
    socket.addEventListener("error", reject, { once: true });
  });
  let nextId = 0;
  const pending = new Map();
  socket.addEventListener("message", ({ data }) => {
    const message = JSON.parse(data);
    const receipt = pending.get(message.id);
    if (!receipt) return;
    pending.delete(message.id);
    clearTimeout(receipt.timer);
    if (message.error) receipt.reject(new Error(message.error.message));
    else receipt.resolve(message.result);
  });
  socket.addEventListener("close", () => {
    for (const receipt of pending.values()) {
      clearTimeout(receipt.timer);
      receipt.reject(new Error("Measurement connection closed"));
    }
    pending.clear();
  });
  function call(method, params) {
    return new Promise((resolve, reject) => {
      const id = ++nextId;
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 30000);
      pending.set(id, { resolve, reject, timer });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }
  return {
    async evaluate(fn, argument) {
      try {
        const result = await call("Runtime.evaluate", {
          expression: `(${fn.toString()})(${JSON.stringify(argument) ?? "undefined"})`,
          awaitPromise: true,
          returnByValue: true,
          objectGroup: "measurement",
        });
        if (result.exceptionDetails)
          throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
        return result.result.value;
      } finally {
        await call("Runtime.releaseObjectGroup", { objectGroup: "measurement" });
      }
    },
    async close() {
      if (socket.readyState === WebSocket.CLOSED) return;
      await new Promise((resolve) => {
        socket.addEventListener("close", resolve, { once: true });
        socket.close();
      });
    },
  };
}
