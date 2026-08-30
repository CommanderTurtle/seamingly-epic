"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");


let extension = null;
const app = {
  graph: { _nodes: [] },
  registerExtension(value) {
    extension = value;
  },
};
const api = { fetchApi() { throw new Error("network is not part of this logic test"); } };
const sourcePath = path.join(__dirname, "..", "web", "comfyshell.js");
const source = fs
  .readFileSync(sourcePath, "utf8")
  .replace(/^import .*;\r?\n/gm, "");
const context = vm.createContext({
  app,
  api,
  console,
  Blob,
  Response,
  TextDecoder,
  Uint8Array,
  DataView,
  JSON,
  Number,
  Math,
  Set,
  Error,
  window: { alert(message) { throw new Error(message); } },
  document: {},
});
new vm.Script(source, { filename: sourcePath }).runInContext(context);
assert.ok(extension, "frontend extension registered");


function mockNode(comfyClass, widgets, outputs = [{ name: "summary", type: "STRING" }]) {
  return {
    comfyClass,
    widgets,
    inputs: [],
    outputs,
    size: [300, 200],
    graph: { setDirtyCanvas() {} },
    setDirtyCanvas() {},
    computeSize() { return this.size; },
    setSize(value) { this.size = value; },
    addWidget(type, name, value, callback, options = {}) {
      const widget = { type, name, value, callback, options };
      this.widgets.push(widget);
      return widget;
    },
    removeWidget(widget) {
      this.widgets.splice(this.widgets.indexOf(widget), 1);
    },
    addInput(name, type, extra = {}) {
      const input = { name, type, ...extra };
      this.inputs.push(input);
      return input;
    },
    removeInput(index) {
      this.inputs.splice(index, 1);
    },
    addOutput(name, type, extra = {}) {
      const output = { name, type, ...extra };
      this.outputs.push(output);
      return output;
    },
    removeOutput(index) {
      this.outputs.splice(index, 1);
    },
  };
}


const count = { name: "temp_value_count", value: 0, callback: null, options: {} };
const saved = { name: "temp_values_json", value: "{}", callback: null, options: {} };
const powerShell = mockNode("NativePowerShell_RunScript", [count, saved]);
extension.nodeCreated(powerShell);
assert.equal(powerShell.inputs.length, 0, "zero count exposes no temp sockets");

count.value = 3;
count.callback(3);
assert.deepEqual(
  powerShell.inputs.map((input) => input.name),
  ["temp1", "temp2", "temp3"],
);
assert.deepEqual(
  powerShell.widgets.filter((widget) => widget.comfyshellTemp).map((widget) => widget.name),
  ["temp1", "temp2", "temp3"],
);
const temp2 = powerShell.widgets.find((widget) => widget.name === "temp2");
temp2.value = 8192;
temp2.callback(8192);
assert.equal(JSON.parse(saved.value).temp2, 8192, "manual fallback persisted");

count.value = 0;
count.callback(0);
assert.equal(powerShell.inputs.length, 0, "lowering to zero removes all temp sockets");
assert.equal(
  powerShell.widgets.filter((widget) => widget.comfyshellTemp).length,
  0,
  "lowering to zero removes all temp widgets",
);

const restoredCount = { name: "temp_value_count", value: 2, callback: null, options: {} };
const restoredState = {
  name: "temp_values_json",
  value: '{"temp1":11,"temp2":22}',
  callback: null,
  options: {},
};
const restored = mockNode("NativePowerShell_RunScript", [restoredCount, restoredState]);
restored.inputs = [
  { name: "temp1", type: "INT" },
  { name: "temp2", type: "INT" },
];
extension.nodeCreated(restored);
assert.equal(restored.inputs.length, 2, "serialized temp sockets are reused, not duplicated");
restoredCount.value = 0;
restoredCount.callback(0);
assert.equal(restored.inputs.length, 0, "serialized temp sockets remain removable after reload");


const snapshot = {
  values: [
    { path: "node.3.seed", label: "#3 KSampler · seed", type: "INT", value: 42 },
    { path: "node.3.cfg", label: "#3 KSampler · cfg", type: "FLOAT", value: 7.5 },
  ],
};
const metadata = mockNode(
  "ComfyShell_ImportWorkflowMetadata",
  [
    { name: "source", value: "fixture", options: {} },
    { name: "snapshot_json", value: JSON.stringify(snapshot), options: {} },
  ],
  [
    { name: "summary", type: "STRING" },
    { name: "value_0", type: "*" },
    { name: "value_1", type: "*" },
    { name: "value_2", type: "*" },
  ],
);
extension.nodeCreated(metadata);
assert.deepEqual(
  metadata.outputs.map((output) => [output.name, output.type]),
  [
    ["summary", "STRING"],
    ["#3 KSampler · seed", "INT"],
    ["#3 KSampler · cfg", "FLOAT"],
  ],
  "only active typed outputs remain visible",
);

console.log("ComfyShell frontend logic tests passed");
