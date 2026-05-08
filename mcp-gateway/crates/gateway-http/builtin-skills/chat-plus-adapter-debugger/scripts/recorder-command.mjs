#!/usr/bin/env node

const action = process.argv[2] || "install";

function installSnippet() {
  const KEY = "__MCP_GATEWAY_NETWORK_RECORDER__";
  const LEGACY_KEY = "__CHAT_PLUS_ADAPTER_DEBUGGER__";
  const existing = window[KEY] || window[LEGACY_KEY];

  if (existing && existing.installed) {
    existing.clear();
    window[KEY] = existing;
    window[LEGACY_KEY] = existing;
    return {
      installed: true,
      reused: true,
      key: KEY,
      size: existing.records.length,
      performanceBaseline: existing.performanceBaseline,
      url: location.href,
      title: document.title,
    };
  }

  const sensitiveHeaders = /^(authorization|cookie|set-cookie|proxy-authorization|x-api-key|api-key)$/i;
  const textContentType = /json|text|event-stream|ndjson|javascript|xml|html|plain|graphql|x-www-form-urlencoded/i;

  const state = {
    installed: true,
    key: KEY,
    records: [],
    seq: 0,
    maxRecords: 500,
    maxPreviewLength: 120000,
    performanceBaseline: performance.getEntriesByType("resource").length,
    clear() {
      this.records.length = 0;
      this.seq = 0;
      this.performanceBaseline = performance.getEntriesByType("resource").length;
    },
    push(record) {
      const saved = {
        id: ++this.seq,
        ts: Date.now(),
        elapsedMs: Math.round(performance.now()),
        pageUrl: location.href,
        ...record,
      };
      this.records.push(saved);
      if (this.records.length > this.maxRecords) this.records.shift();
      return saved.id;
    },
  };

  const safeString = (value) => {
    try {
      return String(value);
    } catch (error) {
      return "[stringify failed: " + ((error && error.message) || error) + "]";
    }
  };

  const truncate = (value, limit = state.maxPreviewLength) => {
    if (value == null) return value;
    const text = typeof value === "string" ? value : safeString(value);
    return text.length > limit ? text.slice(0, limit) + "...[truncated]" : text;
  };

  const describeBinary = (name, value, type) => {
    const size = value && typeof value.size === "number"
      ? value.size
      : value && typeof value.byteLength === "number"
        ? value.byteLength
        : value && value.buffer && typeof value.byteLength === "number"
          ? value.byteLength
          : "unknown";
    return "[" + name + " " + (type || "unknown") + " " + size + "]";
  };

  const previewBodySync = (value) => {
    try {
      if (value == null) return null;
      if (typeof value === "string") return truncate(value);
      if (typeof URLSearchParams !== "undefined" && value instanceof URLSearchParams) return truncate(value.toString());
      if (typeof FormData !== "undefined" && value instanceof FormData) {
        const entries = [];
        value.forEach((entryValue, key) => {
          const preview = typeof File !== "undefined" && entryValue instanceof File
            ? describeBinary("File", entryValue, entryValue.type || entryValue.name)
            : typeof Blob !== "undefined" && entryValue instanceof Blob
              ? describeBinary("Blob", entryValue, entryValue.type)
              : truncate(entryValue, 2000);
          entries.push({ key, value: preview });
        });
        return { kind: "FormData", entries };
      }
      if (typeof Blob !== "undefined" && value instanceof Blob) return describeBinary("Blob", value, value.type);
      if (typeof ArrayBuffer !== "undefined" && value instanceof ArrayBuffer) return describeBinary("ArrayBuffer", value);
      if (typeof ArrayBuffer !== "undefined" && ArrayBuffer.isView(value)) return describeBinary(value.constructor && value.constructor.name || "TypedArray", value);
      if (typeof ReadableStream !== "undefined" && value instanceof ReadableStream) return "[ReadableStream body omitted]";
      return truncate(value);
    } catch (error) {
      return "[preview failed: " + ((error && error.message) || error) + "]";
    }
  };

  const previewBodyAsync = async (value) => {
    try {
      if (value == null) return null;
      if (typeof Request !== "undefined" && value instanceof Request) {
        if (value.bodyUsed) return "[Request body already used]";
        return truncate(await value.clone().text());
      }
      if (typeof Blob !== "undefined" && value instanceof Blob && textContentType.test(value.type || "")) {
        return truncate(await value.text());
      }
      return previewBodySync(value);
    } catch (error) {
      return "[preview failed: " + ((error && error.message) || error) + "]";
    }
  };

  const headersObject = (headers) => {
    const result = {};
    try {
      new Headers(headers || {}).forEach((value, key) => {
        result[key] = sensitiveHeaders.test(key) ? "[redacted]" : value;
      });
    } catch (_) {}
    return result;
  };

  const fetchMetadata = (input, init = {}) => {
    const isRequest = typeof Request !== "undefined" && input instanceof Request;
    const initHeaders = init && init.headers ? headersObject(init.headers) : {};
    const requestHeaders = isRequest ? headersObject(input.headers) : {};
    return {
      url: typeof input === "string" || (typeof URL !== "undefined" && input instanceof URL) ? safeString(input) : input && input.url,
      method: safeString((init && init.method) || (input && input.method) || "GET").toUpperCase(),
      requestHeaders: { ...requestHeaders, ...initHeaders },
      requestBodySource: init && "body" in init ? "init.body" : isRequest ? "Request" : "none",
      requestBodyValue: init && "body" in init ? init.body : isRequest ? input : null,
      mode: (init && init.mode) || (isRequest && input.mode) || undefined,
      credentials: (init && init.credentials) || (isRequest && input.credentials) || undefined,
      cache: (init && init.cache) || (isRequest && input.cache) || undefined,
    };
  };

  const originalBeacon = navigator.sendBeacon && navigator.sendBeacon.bind(navigator);
  if (originalBeacon) {
    navigator.sendBeacon = function recordedSendBeacon(url, data) {
      state.push({
        kind: "beacon",
        phase: "request",
        url: safeString(url || ""),
        method: "POST",
        requestBodyPreview: previewBodySync(data),
      });
      return originalBeacon(url, data);
    };
  }

  const originalFetch = window.fetch;
  if (originalFetch) {
    window.fetch = async function recordedFetch(input, init = {}) {
      const meta = fetchMetadata(input, init);
      const id = state.push({
        kind: "fetch",
        phase: "request",
        url: meta.url,
        method: meta.method,
        requestHeaders: meta.requestHeaders,
        requestBodySource: meta.requestBodySource,
        requestBodyPreview: "[pending]",
        mode: meta.mode,
        credentials: meta.credentials,
        cache: meta.cache,
      });

      previewBodyAsync(meta.requestBodyValue).then(
        (preview) => state.push({
          kind: "fetch",
          phase: "request-body",
          requestId: id,
          url: meta.url,
          method: meta.method,
          requestBodySource: meta.requestBodySource,
          requestBodyPreview: preview,
        }),
        (error) => state.push({
          kind: "fetch",
          phase: "request-body-error",
          requestId: id,
          url: meta.url,
          method: meta.method,
          error: safeString((error && error.message) || error),
        }),
      );

      try {
        const response = await originalFetch.apply(this, arguments);
        const contentType = response.headers && response.headers.get && response.headers.get("content-type") || "";
        const status = response.status;
        const responseHeaders = headersObject(response.headers);

        if (textContentType.test(contentType || "")) {
          response.clone().text().then(
            (text) => state.push({
              kind: "fetch",
              phase: "response",
              requestId: id,
              url: meta.url,
              method: meta.method,
              status,
              contentType,
              responseHeaders,
              responseText: truncate(text),
            }),
            (error) => state.push({
              kind: "fetch",
              phase: "response-error",
              requestId: id,
              url: meta.url,
              method: meta.method,
              status,
              contentType,
              responseHeaders,
              error: safeString((error && error.message) || error),
            }),
          );
        } else {
          state.push({
            kind: "fetch",
            phase: "response",
            requestId: id,
            url: meta.url,
            method: meta.method,
            status,
            contentType,
            responseHeaders,
            responseText: "[non-text response omitted]",
          });
        }

        return response;
      } catch (error) {
        state.push({
          kind: "fetch",
          phase: "request-error",
          requestId: id,
          url: meta.url,
          method: meta.method,
          error: safeString((error && error.message) || error),
        });
        throw error;
      }
    };
  }

  const OriginalXHR = window.XMLHttpRequest;
  if (OriginalXHR) {
    function RecordedXMLHttpRequest() {
      const xhr = new OriginalXHR();
      const meta = { requestHeaders: {} };
      const open = xhr.open;
      const send = xhr.send;
      const setRequestHeader = xhr.setRequestHeader;

      xhr.open = function recordedXhrOpen(method, url) {
        meta.method = safeString(method || "GET").toUpperCase();
        meta.url = safeString(url || "");
        meta.async = arguments.length < 3 ? true : arguments[2];
        return open.apply(xhr, arguments);
      };

      xhr.setRequestHeader = function recordedXhrSetRequestHeader(key, value) {
        meta.requestHeaders[key] = sensitiveHeaders.test(key) ? "[redacted]" : safeString(value);
        return setRequestHeader.apply(xhr, arguments);
      };

      xhr.send = function recordedXhrSend(body) {
        const id = state.push({
          kind: "xhr",
          phase: "request",
          url: meta.url,
          method: meta.method || "GET",
          requestHeaders: meta.requestHeaders,
          requestBodyPreview: previewBodySync(body),
          responseType: xhr.responseType || "",
          async: meta.async,
        });

        xhr.addEventListener("loadend", () => {
          let responseText = "";
          try {
            if (!xhr.responseType || xhr.responseType === "text") {
              responseText = truncate(xhr.responseText);
            } else if (xhr.responseType === "json") {
              responseText = truncate(JSON.stringify(xhr.response));
            } else {
              responseText = "[non-text response omitted: " + xhr.responseType + "]";
            }
          } catch (_) {
            responseText = "[non-text response omitted]";
          }

          state.push({
            kind: "xhr",
            phase: "response",
            requestId: id,
            url: meta.url,
            method: meta.method || "GET",
            status: xhr.status,
            contentType: xhr.getResponseHeader("content-type") || "",
            responseText,
          });
        });

        return send.apply(xhr, arguments);
      };

      return xhr;
    }

    Object.setPrototypeOf(RecordedXMLHttpRequest, OriginalXHR);
    RecordedXMLHttpRequest.prototype = OriginalXHR.prototype;
    window.XMLHttpRequest = RecordedXMLHttpRequest;
  }

  const OriginalWebSocket = window.WebSocket;
  if (OriginalWebSocket) {
    function RecordedWebSocket(url, protocols) {
      const ws = protocols === undefined ? new OriginalWebSocket(url) : new OriginalWebSocket(url, protocols);
      const socketId = state.push({
        kind: "websocket",
        phase: "open",
        url: safeString(url),
        protocols: protocols == null ? undefined : previewBodySync(protocols),
      });
      const send = ws.send;

      ws.send = function recordedWebSocketSend(data) {
        state.push({
          kind: "websocket",
          phase: "outbound",
          requestId: socketId,
          url: safeString(url),
          dataPreview: previewBodySync(data),
        });
        return send.apply(ws, arguments);
      };

      ws.addEventListener("message", (event) => {
        state.push({
          kind: "websocket",
          phase: "inbound",
          requestId: socketId,
          url: safeString(url),
          dataPreview: previewBodySync(event.data),
        });
      });

      ws.addEventListener("close", (event) => {
        state.push({
          kind: "websocket",
          phase: "close",
          requestId: socketId,
          url: safeString(url),
          code: event.code,
          reason: event.reason,
          wasClean: event.wasClean,
        });
      });

      return ws;
    }

    Object.setPrototypeOf(RecordedWebSocket, OriginalWebSocket);
    RecordedWebSocket.prototype = OriginalWebSocket.prototype;
    window.WebSocket = RecordedWebSocket;
  }

  const OriginalEventSource = window.EventSource;
  if (OriginalEventSource) {
    function RecordedEventSource(url, config) {
      const es = new OriginalEventSource(url, config);
      const sourceId = state.push({
        kind: "eventsource",
        phase: "open",
        url: safeString(url),
        withCredentials: config && config.withCredentials,
      });
      const watchedTypes = new Set();
      const originalAddEventListener = es.addEventListener;

      const watchType = (type) => {
        if (watchedTypes.has(type)) return;
        watchedTypes.add(type);
        originalAddEventListener.call(es, type, (event) => {
          state.push({
            kind: "eventsource",
            phase: "message",
            requestId: sourceId,
            url: safeString(url),
            eventType: type,
            dataPreview: previewBodySync(event.data),
          });
        });
      };

      es.addEventListener = function recordedEventSourceAddEventListener(type) {
        watchType(type);
        return originalAddEventListener.apply(es, arguments);
      };

      watchType("message");
      watchType("error");
      watchType("open");
      return es;
    }

    Object.setPrototypeOf(RecordedEventSource, OriginalEventSource);
    RecordedEventSource.prototype = OriginalEventSource.prototype;
    window.EventSource = RecordedEventSource;
  }

  window[KEY] = state;
  window[LEGACY_KEY] = state;

  return {
    installed: true,
    reused: false,
    key: KEY,
    size: 0,
    performanceBaseline: state.performanceBaseline,
    url: location.href,
    title: document.title,
    transports: ["fetch", "xhr", "beacon", "websocket", "eventsource"],
  };
}

function clearSnippet() {
  const state = window.__MCP_GATEWAY_NETWORK_RECORDER__ || window.__CHAT_PLUS_ADAPTER_DEBUGGER__;
  if (state && state.clear) state.clear();
  return {
    cleared: true,
    size: state && state.records ? state.records.length : 0,
    performanceBaseline: state && state.performanceBaseline || 0,
    url: location.href,
    title: document.title,
  };
}

function recordsSnippet() {
  const state = window.__MCP_GATEWAY_NETWORK_RECORDER__ || window.__CHAT_PLUS_ADAPTER_DEBUGGER__;
  const records = state && state.records || [];
  const limit = 4000;
  const shortLimit = 500;

  const trim = (value, max) => {
    if (typeof value !== "string") return value;
    return value.length > max ? value.slice(0, max) + "...[truncated]" : value;
  };

  const compact = (record, max) => ({
    id: record.id,
    ts: record.ts,
    elapsedMs: record.elapsedMs,
    kind: record.kind,
    phase: record.phase,
    url: record.url,
    method: record.method,
    status: record.status,
    contentType: record.contentType,
    requestId: record.requestId,
    requestBodySource: record.requestBodySource,
    requestHeaders: record.requestHeaders,
    responseHeaders: record.responseHeaders,
    responseType: record.responseType,
    eventType: record.eventType,
    code: record.code,
    error: record.error,
    requestBodyPreview: trim(record.requestBodyPreview, max),
    responseText: trim(record.responseText, max),
    dataPreview: trim(record.dataPreview, max),
  });

  const score = (record) => {
    const haystack = JSON.stringify({
      url: record.url,
      method: record.method,
      contentType: record.contentType,
      requestBodyPreview: record.requestBodyPreview,
      responseText: record.responseText,
      dataPreview: record.dataPreview,
    }).toLowerCase();
    let value = 0;
    const reasons = [];

    if (/^(post|put|patch|delete)$/i.test(record.method || "")) {
      value += 30;
      reasons.push("mutation-method");
    }
    if (record.phase === "request-body" && record.requestBodyPreview) {
      value += 25;
      reasons.push("has-request-body");
    }
    if (record.phase === "response" && record.responseText && !/^\[non-text/.test(record.responseText)) {
      value += 25;
      reasons.push("text-response");
    }
    if (record.kind === "websocket" && (record.phase === "outbound" || record.phase === "inbound")) {
      value += 20;
      reasons.push("websocket-frame");
    }
    if (/json|event-stream|ndjson|graphql|x-www-form-urlencoded|connect|protobuf|grpc|rpc/i.test(record.contentType || "")) {
      value += 20;
      reasons.push("api-content-type");
    }
    if (/\/(api|rpc|graphql|v\d+|chat|conversation|completion|message|stream|events?)(\/|$|\?)/i.test(record.url || "")) {
      value += 15;
      reasons.push("api-like-url");
    }
    if (/CHATPLUS_ADAPTER_PROBE|probe|prompt|query|question|message|content|delta|append|answer|response|stream|operationname|variables/i.test(haystack)) {
      value += 10;
      reasons.push("payload-keyword");
    }
    if (/\.(js|css|png|jpg|jpeg|gif|webp|svg|ico|woff2?|ttf|map)(\?|$)/i.test(record.url || "")) {
      value -= 40;
      reasons.push("static-asset");
    }
    if (/analytics|telemetry|metrics|beacon|log|sentry|segment|gtag|collect/i.test(record.url || "")) {
      value -= 20;
      reasons.push("telemetry-like");
    }

    return { score: value, reasons };
  };

  const summaries = records.map((record) => compact(record, shortLimit));
  const candidates = records
    .map((record) => ({ ...compact(record, limit), ...score(record) }))
    .filter((record) => record.score > 0)
    .sort((a, b) => b.score - a.score || a.id - b.id)
    .slice(0, 80);

  return {
    count: records.length,
    returned: summaries.length,
    candidateCount: candidates.length,
    records: summaries,
    candidates,
  };
}

function recordsFullSnippet() {
  const state = window.__MCP_GATEWAY_NETWORK_RECORDER__ || window.__CHAT_PLUS_ADAPTER_DEBUGGER__;
  return {
    count: state && state.records ? state.records.length : 0,
    records: state && state.records || [],
  };
}

function performanceSnippet() {
  const state = window.__MCP_GATEWAY_NETWORK_RECORDER__ || window.__CHAT_PLUS_ADAPTER_DEBUGGER__;
  const baseline = state && state.performanceBaseline || 0;
  const all = performance.getEntriesByType("resource");
  const entries = all.slice(baseline).map((entry, index) => {
    const type = entry.initiatorType || "";
    const name = entry.name || "";
    let suggestedNetworkType = "fetch";
    if (/xmlhttprequest/i.test(type)) suggestedNetworkType = "xhr";
    if (/websocket/i.test(type)) suggestedNetworkType = "websocket";

    let score = 0;
    const reasons = [];
    if (/fetch|xmlhttprequest|beacon|eventsource|websocket/i.test(type)) {
      score += 25;
      reasons.push("script-initiated");
    }
    if (/\/(api|rpc|graphql|v\d+|chat|conversation|completion|message|stream|events?)(\/|$|\?)/i.test(name)) {
      score += 20;
      reasons.push("api-like-url");
    }
    if ((entry.transferSize || entry.encodedBodySize || entry.decodedBodySize || 0) > 0) {
      score += 10;
      reasons.push("has-body-size");
    }
    if (/\.(js|css|png|jpg|jpeg|gif|webp|svg|ico|woff2?|ttf|map)(\?|$)/i.test(name)) {
      score -= 40;
      reasons.push("static-asset");
    }
    if (/analytics|telemetry|metrics|beacon|log|sentry|segment|gtag|collect/i.test(name)) {
      score -= 15;
      reasons.push("telemetry-like");
    }

    return {
      index,
      name,
      initiatorType: type,
      suggestedNetworkType,
      startTime: Math.round(entry.startTime),
      duration: Math.round(entry.duration),
      transferSize: entry.transferSize,
      encodedBodySize: entry.encodedBodySize,
      decodedBodySize: entry.decodedBodySize,
      score,
      reasons,
    };
  });

  return {
    baseline,
    total: all.length,
    count: entries.length,
    entries,
    candidates: entries
      .filter((entry) => entry.score > 0)
      .sort((a, b) => b.score - a.score || a.index - b.index)
      .slice(0, 80),
  };
}

const asEvalFunction = (fn) => fn
  .toString()
  .replace(/^function\s+[A-Za-z0-9_$]+\s*\(/, "function(")
  .replace(/\s+/g, " ")
  .trim();

const snippets = {
  install: asEvalFunction(installSnippet),
  clear: asEvalFunction(clearSnippet),
  records: asEvalFunction(recordsSnippet),
  "records-full": asEvalFunction(recordsFullSnippet),
  performance: asEvalFunction(performanceSnippet),
};

if (action === "help" || action === "--help" || action === "-h") {
  console.log("Usage: node scripts/recorder-command.mjs <install|clear|records|records-full|performance|raw-install|raw-clear|raw-records|raw-records-full|raw-performance>");
  process.exit(0);
}

const raw = action.startsWith("raw-");
const key = raw ? action.slice(4) : action;
const snippet = snippets[key];

if (!snippet) {
  console.error(`Unknown action: ${action}`);
  process.exit(2);
}

if (raw) {
  console.log(snippet);
} else {
  if (snippet.includes("'")) {
    console.error("Generated eval snippet contains a single quote and cannot be safely quoted for the gateway command parser.");
    process.exit(3);
  }
  console.log(`eval '${snippet}'`);
}
