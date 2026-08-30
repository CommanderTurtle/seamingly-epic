import { app } from "../../scripts/app.js";
import { api } from "../../scripts/api.js";

const METADATA_NODE = "ComfyShell_ImportWorkflowMetadata";
const POWERSHELL_NODE = "NativePowerShell_RunScript";
const MAX_TEMP_VALUES = 32;
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];
const PNG_TEXT_TYPES = new Set(["tEXt", "zTXt", "iTXt"]);

function findWidget(node, name) {
  return node.widgets?.find((widget) => widget.name === name);
}

function markChanged(node) {
  node.graph?.setDirtyCanvas?.(true, true);
  node.setDirtyCanvas?.(true, true);
}

function parseJsonObject(value, fallback = {}) {
  try {
    const parsed = JSON.parse(value || "{}");
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? parsed
      : fallback;
  } catch (_error) {
    return fallback;
  }
}

function applyMetadataOutputs(node) {
  const snapshotWidget = findWidget(node, "snapshot_json");
  const snapshot = parseJsonObject(snapshotWidget?.value, null);
  const values = Array.isArray(snapshot?.values) ? snapshot.values : [];

  const current = node.outputs?.slice(1) || [];
  const alreadyMatches =
    current.length === values.length &&
    current.every(
      (output, index) =>
        (output.comfyshellPath
          ? output.comfyshellPath === values[index].path
          : output.name === values[index].label) &&
        output.type === values[index].type,
    );
  let changed = false;

  if (!alreadyMatches) {
    for (let index = (node.outputs?.length || 1) - 1; index >= 1; index -= 1) {
      node.removeOutput(index);
      changed = true;
    }
    for (const descriptor of values) {
      node.addOutput(descriptor.label, descriptor.type, {
        comfyshellPath: descriptor.path,
        label: descriptor.label,
      });
      changed = true;
    }
  } else {
    current.forEach((output, index) => {
      if (output.name !== values[index].label) changed = true;
      output.comfyshellPath = values[index].path;
      output.name = values[index].label;
      output.label = values[index].label;
    });
  }
  if (changed) markChanged(node);
}

async function inflate(bytes) {
  if (typeof DecompressionStream !== "function") {
    throw new Error("This browser cannot decompress PNG metadata (DecompressionStream missing).");
  }
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("deflate"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

function nullIndex(bytes, start = 0) {
  for (let index = start; index < bytes.length; index += 1) {
    if (bytes[index] === 0) return index;
  }
  return -1;
}

async function decodePngText(type, bytes) {
  const latin1 = new TextDecoder("latin1");
  const utf8 = new TextDecoder("utf-8");
  const firstNull = nullIndex(bytes);
  if (firstNull < 0) throw new Error(`Malformed PNG ${type} chunk.`);
  const key = latin1.decode(bytes.slice(0, firstNull));
  let rest = bytes.slice(firstNull + 1);

  if (type === "tEXt") return [key, latin1.decode(rest)];
  if (type === "zTXt") {
    if (rest.length < 2 || rest[0] !== 0) {
      throw new Error("Unsupported PNG zTXt compression method.");
    }
    return [key, latin1.decode(await inflate(rest.slice(1)))];
  }

  if (rest.length < 2 || ![0, 1].includes(rest[0]) || rest[1] !== 0) {
    throw new Error("Unsupported PNG iTXt compression settings.");
  }
  const compressed = rest[0] === 1;
  rest = rest.slice(2);
  const languageEnd = nullIndex(rest);
  if (languageEnd < 0) throw new Error("Malformed PNG iTXt language tag.");
  rest = rest.slice(languageEnd + 1);
  const translatedEnd = nullIndex(rest);
  if (translatedEnd < 0) throw new Error("Malformed PNG iTXt translated keyword.");
  rest = rest.slice(translatedEnd + 1);
  return [key, utf8.decode(compressed ? await inflate(rest) : rest)];
}

async function readPngMetadata(file) {
  const signature = new Uint8Array(await file.slice(0, 8).arrayBuffer());
  if (!PNG_SIGNATURE.every((value, index) => signature[index] === value)) {
    throw new Error(`${file.name} is not a PNG file.`);
  }

  const metadata = {};
  let imageWidth = null;
  let imageHeight = null;
  let offset = 8;
  let textBytes = 0;
  const ascii = new TextDecoder("ascii");

  while (offset + 12 <= file.size) {
    const header = new Uint8Array(await file.slice(offset, offset + 8).arrayBuffer());
    const view = new DataView(header.buffer, header.byteOffset, header.byteLength);
    const length = view.getUint32(0, false);
    const type = ascii.decode(header.slice(4, 8));
    const dataStart = offset + 8;
    const dataEnd = dataStart + length;
    if (dataEnd + 4 > file.size) throw new Error(`Truncated PNG ${type} chunk.`);

    if (type === "IHDR") {
      const data = new DataView(await file.slice(dataStart, dataStart + 8).arrayBuffer());
      imageWidth = data.getUint32(0, false);
      imageHeight = data.getUint32(4, false);
    } else if (PNG_TEXT_TYPES.has(type)) {
      textBytes += length;
      if (textBytes > 64 * 1024 * 1024) {
        throw new Error("PNG textual metadata exceeds 64 MiB safety limit.");
      }
      const data = new Uint8Array(await file.slice(dataStart, dataEnd).arrayBuffer());
      const [key, value] = await decodePngText(type, data);
      metadata[key] = value;
    }

    offset = dataEnd + 4;
    if (type === "IEND") break;
  }
  return { metadata, imageWidth, imageHeight };
}

async function inspectRequest(body) {
  const response = await api.fetchApi("/comfyshell/inspect", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const result = await response.json();
  if (!response.ok || !result.ok) {
    throw new Error(result.error || `Inspection failed (${response.status}).`);
  }
  return result.snapshot;
}

function applySnapshot(node, snapshot) {
  const sourceWidget = findWidget(node, "source");
  const snapshotWidget = findWidget(node, "snapshot_json");
  if (!sourceWidget || !snapshotWidget) throw new Error("ComfyShell widgets are unavailable.");
  sourceWidget.value = snapshot.source_input;
  snapshotWidget.value = JSON.stringify(snapshot);
  applyMetadataOutputs(node);
}

async function inspectCurrentSource(node) {
  const sourceWidget = findWidget(node, "source");
  const source = String(sourceWidget?.value || "").trim();
  if (!source) throw new Error("Paste JSON or enter a .json/.png path first.");
  const snapshotWidget = findWidget(node, "snapshot_json");
  const saved = parseJsonObject(snapshotWidget?.value, null);
  if (saved?.source_input === source && Array.isArray(saved.values)) {
    applyMetadataOutputs(node);
    return;
  }
  applySnapshot(node, await inspectRequest({ source }));
}

async function inspectBrowserFile(node, file) {
  const sourceInput = `browser:${file.name}:${file.size}:${file.lastModified}`;
  if (file.name.toLowerCase().endsWith(".json")) {
    const payload = JSON.parse(await file.text());
    applySnapshot(
      node,
      await inspectRequest({ payload, source_name: file.name, source_input: sourceInput }),
    );
    return;
  }
  const { metadata, imageWidth, imageHeight } = await readPngMetadata(file);
  applySnapshot(
    node,
    await inspectRequest({
      payload: { metadata },
      source_name: file.name,
      source_input: sourceInput,
      image_width: imageWidth,
      image_height: imageHeight,
    }),
  );
}

function reportError(error) {
  const message = error instanceof Error ? error.message : String(error);
  window.alert(`ComfyShell: ${message}`);
}

function installMetadataUi(node) {
  if (node.comfyshellMetadataUi) return;
  node.comfyshellMetadataUi = true;

  const loadButton = node.addWidget("button", "Load JSON / PNG…", null, () => {
    const picker = document.createElement("input");
    picker.type = "file";
    picker.accept = ".json,.png,application/json,image/png";
    picker.addEventListener("change", async () => {
      const file = picker.files?.[0];
      if (!file) return;
      try {
        await inspectBrowserFile(node, file);
      } catch (error) {
        reportError(error);
      }
    });
    picker.click();
  });
  loadButton.serialize = false;

  const inspectButton = node.addWidget("button", "Inspect / rebuild outputs", null, async () => {
    try {
      await inspectCurrentSource(node);
    } catch (error) {
      reportError(error);
    }
  });
  inspectButton.serialize = false;
  applyMetadataOutputs(node);
}

function readTempState(node) {
  return parseJsonObject(findWidget(node, "temp_values_json")?.value, {});
}

function writeTempState(node, state) {
  const widget = findWidget(node, "temp_values_json");
  if (!widget) return false;
  const encoded = JSON.stringify(state);
  if (widget.value === encoded) return false;
  widget.value = encoded;
  return true;
}

function syncTempInputs(node) {
  const countWidget = findWidget(node, "temp_value_count");
  if (!countWidget) return;
  const desired = Math.max(0, Math.min(MAX_TEMP_VALUES, Math.trunc(Number(countWidget.value) || 0)));
  const state = readTempState(node);
  let changed = false;

  // Serialized workflows retain dynamic slot names but not extension-only
  // marker properties. Reclaim those slots before applying the new count.
  for (const input of node.inputs || []) {
    const match = /^temp(\d+)$/.exec(input.name || "");
    if (match && Number(match[1]) <= MAX_TEMP_VALUES) input.comfyshellTemp = true;
  }

  for (const widget of node.widgets || []) {
    if (widget.comfyshellTemp) state[widget.name] = Math.trunc(Number(widget.value) || 0);
  }

  for (let index = (node.inputs?.length || 0) - 1; index >= 0; index -= 1) {
    const input = node.inputs[index];
    const match = input?.comfyshellTemp && /^temp(\d+)$/.exec(input.name);
    if (match && Number(match[1]) > desired) {
      node.removeInput(index);
      changed = true;
    }
  }
  for (const widget of [...(node.widgets || [])]) {
    const match = widget.comfyshellTemp && /^temp(\d+)$/.exec(widget.name);
    if (match && Number(match[1]) > desired) {
      node.removeWidget(widget);
      changed = true;
    }
  }

  for (let index = 1; index <= desired; index += 1) {
    const name = `temp${index}`;
    if (!findWidget(node, name)) {
      const widget = node.addWidget(
        "number",
        name,
        Number.isSafeInteger(state[name]) ? state[name] : 0,
        (value) => {
          const current = readTempState(node);
          current[name] = Math.trunc(Number(value) || 0);
          if (writeTempState(node, current)) markChanged(node);
        },
        { precision: 0, step: 1 },
      );
      // Keep dynamic widget values in API prompts, but persist them once in
      // temp_values_json rather than changing positional workflow widget order.
      widget.serialize = false;
      widget.comfyshellTemp = true;
      changed = true;
    }
    if (!node.inputs?.some((input) => input.name === name)) {
      node.addInput(name, "INT", {
        label: `$${name}`,
        comfyshellTemp: true,
      });
      changed = true;
    }
  }

  changed = writeTempState(node, state) || changed;
  if (changed) markChanged(node);
}

function installPowerShellUi(node) {
  if (node.comfyshellPowerShellUi) return;
  node.comfyshellPowerShellUi = true;
  const countWidget = findWidget(node, "temp_value_count");
  if (countWidget) {
    const originalCallback = countWidget.callback;
    countWidget.callback = function (...args) {
      const result = originalCallback?.apply(this, args);
      syncTempInputs(node);
      return result;
    };
  }
  syncTempInputs(node);
}

app.registerExtension({
  name: "ComfyShell.DynamicValues",

  async beforeRegisterNodeDef(nodeType, nodeData) {
    const nodeId = nodeData?.name || nodeType.comfyClass;
    if (nodeId !== METADATA_NODE && nodeId !== POWERSHELL_NODE) return;
    const originalConfigure = nodeType.prototype.onConfigure;
    nodeType.prototype.onConfigure = function (...args) {
      const result = originalConfigure?.apply(this, args);
      if (nodeId === METADATA_NODE) {
        installMetadataUi(this);
        applyMetadataOutputs(this);
      } else {
        installPowerShellUi(this);
        syncTempInputs(this);
      }
      return result;
    };
  },

  async nodeCreated(node) {
    if (node.comfyClass === METADATA_NODE) installMetadataUi(node);
    if (node.comfyClass === POWERSHELL_NODE) installPowerShellUi(node);
  },

  async afterConfigureGraph() {
    for (const node of app.graph?._nodes || []) {
      if (node.comfyClass === METADATA_NODE) applyMetadataOutputs(node);
      if (node.comfyClass === POWERSHELL_NODE) syncTempInputs(node);
    }
  },
});
