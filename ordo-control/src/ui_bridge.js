/**
 * Ordo UI extension bridge.
 *
 * Every sandboxed extension iframe loads this file once as
 *   <script src="/api/ui-extensions/_bridge.js"></script>
 * and then uses `window.ordo.*` to talk to the parent studio. The
 * parent mediates everything â€” validating manifest permissions and
 * forwarding permitted requests to the control API. Extension code
 * never touches the runtime directly.
 *
 * All messages use a compact JSON envelope. Every request the
 * extension sends gets either a `result` or `error` message back,
 * correlated by `id`. Events (from the parent) have no id and
 * arrive unsolicited.
 */
(function () {
  if (window.ordo) return; // loaded twice? ignore.

  var nextId = 1;
  var pending = new Map();
  var eventHandlers = new Map(); // topic string â†’ Set<callback>

  function postToParent(message) {
    // Sandboxed iframes with `allow-scripts` but no `allow-same-origin`
    // have an opaque origin, so we use "*" for the target origin. The
    // parent validates us by comparing MessageEvent.source against the
    // iframe's contentWindow â€” that's the trust anchor, not origin.
    window.parent.postMessage(message, "*");
  }

  function request(method, params) {
    return new Promise(function (resolve, reject) {
      var id = nextId++;
      pending.set(id, { resolve: resolve, reject: reject });
      postToParent({ id: id, type: "call", method: method, params: params || {} });
    });
  }

  window.addEventListener("message", function (event) {
    // Only trust messages from the parent frame.
    if (event.source !== window.parent) return;
    var message = event.data;
    if (!message || typeof message !== "object") return;

    if (typeof message.id === "number") {
      var slot = pending.get(message.id);
      if (!slot) return;
      pending.delete(message.id);
      if (message.type === "result") {
        slot.resolve(message.result);
      } else if (message.type === "error") {
        slot.reject(new Error(message.error || "ordo bridge error"));
      }
      return;
    }

    if (message.type === "event" && typeof message.topic === "string") {
      var handlers = eventHandlers.get(message.topic) || new Set();
      handlers.forEach(function (handler) {
        try {
          handler(message.payload);
        } catch (err) {
          console.error("[ordo bridge] event handler threw:", err);
        }
      });
      // Also fan out to wildcard subscribers.
      eventHandlers.forEach(function (handlers, pattern) {
        if (pattern === message.topic) return;
        if (pattern.endsWith("*") && message.topic.startsWith(pattern.slice(0, -1))) {
          handlers.forEach(function (handler) {
            try {
              handler(message.payload);
            } catch (err) {
              console.error("[ordo bridge] event handler threw:", err);
            }
          });
        }
      });
    }
  });

  window.ordo = Object.freeze({
    version: "0.1.0",

    tools: Object.freeze({
      /** Invoke a capability. Rejects if the extension manifest
       *  doesn't permit it. */
      call: function (capability, args) {
        return request("tools.call", {
          capability: capability,
          arguments: args || {},
        });
      },
      /** Return the full capability inventory (subject to same
       *  permission check as call). */
      list: function () {
        return request("tools.list", {});
      },
    }),

    events: Object.freeze({
      /** Subscribe to a topic (or glob ending in `*`). The parent
       *  enforces the manifest's permissions.subscribe_events list. */
      on: function (topic, handler) {
        if (typeof topic !== "string" || typeof handler !== "function") {
          throw new TypeError("ordo.events.on(topic: string, handler: function)");
        }
        var handlers = eventHandlers.get(topic);
        if (!handlers) {
          handlers = new Set();
          eventHandlers.set(topic, handlers);
          // Notify parent to start forwarding (idempotent if already
          // subscribed at parent level).
          postToParent({ type: "subscribe", topic: topic });
        }
        handlers.add(handler);
        return function () {
          handlers.delete(handler);
          if (handlers.size === 0) {
            eventHandlers.delete(topic);
            postToParent({ type: "unsubscribe", topic: topic });
          }
        };
      },
    }),

    ui: Object.freeze({
      /** Ask the parent studio to close the extension tab. */
      close: function () {
        postToParent({ type: "ui.close" });
      },
      /** Ask the parent studio to show a toast. */
      toast: function (text, tone) {
        postToParent({ type: "ui.toast", text: text, tone: tone || "info" });
      },
    }),

    /** Extension manifest metadata (injected by the parent on ready). */
    manifest: null,

    ready: new Promise(function (resolve) {
      window.addEventListener("message", function onReady(event) {
        if (event.source !== window.parent) return;
        if (event.data && event.data.type === "ready") {
          window.ordo.manifest = event.data.manifest || null;
          window.removeEventListener("message", onReady);
          resolve(window.ordo.manifest);
        }
      });
      // Signal the parent we're alive so it can send `ready`.
      postToParent({ type: "hello" });
    }),
  });
})();
