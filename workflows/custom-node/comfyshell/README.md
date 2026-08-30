# ComfyShell

ComfyShell adds two deliberately small developer nodes to ComfyUI:

- **ComfyShell → Workflow → Import Workflow Metadata** turns an old workflow
  JSON or generated Comfy PNG into typed, connectable setting outputs.
- **ComfyShell → Automation → Run PowerShell (ComfyShell)** forms a completion
  barrier and runs one fresh, hidden `pwsh` process after every connected
  dependency finishes.

It uses ComfyUI's modern V3 node API. It does not ship a shell, retain shell
state, decode image pixels, or modify an imported workflow.

## Install

Copy this repository to `ComfyUI/custom_nodes/comfyshell`, then restart
ComfyUI and refresh the browser. There are no additional Python packages.
PowerShell execution expects `pwsh` on `PATH`; an absolute `pwsh.exe` path can
instead be entered under **Advanced**.

## Import Workflow Metadata

The importer accepts any of these sources:

1. pasted ComfyUI API/workflow JSON;
2. a local `.json` path;
3. a local generated `.png` path;
4. a JSON or PNG chosen with **Load JSON / PNG…** in the node.

For a pasted value or server-local path, press **Inspect / rebuild outputs**.
The file chooser performs PNG chunk inspection in the browser and sends only
the embedded metadata to ComfyUI. The server-side path reader likewise seeks
past `IDAT`: neither route decompresses or materializes the image pixels, even
for an enormous generation. Use the PNG file/path rather than a decoded
`IMAGE` tensor; ComfyUI tensors no longer carry the original textual chunks.

The node understands both API prompts and modern UI workflows, including
`widgets_values_named`. When a PNG contains both `prompt` and `workflow`, API
execution values are authoritative and UI data supplies human-friendly node
titles plus UI-only values. Primitive fields inside nested LoRA/switch/list
settings are flattened into separate outputs.

Useful inferred outputs appear first:

- source PNG width and height;
- the first executing sampler's node ID and class;
- the width and height found by following that sampler's latent input upstream.

They are followed by exact stored values such as seeds, steps, CFG, sampler and
scheduler names, model/LoRA names and weights, prompt strings, booleans, and
other widget values. Output sockets use `INT`, `FLOAT`, `BOOLEAN`, or `STRING`
as appropriate and are labelled with their original node and field.

ComfyUI backend output positions are schema-indexed, so ComfyShell reserves a
stable bank and exposes only the active sockets in the browser. The bank holds
the first 512 useful scalar values; the `summary` output reports when a very
large workflow was truncated. Reinspection preserves links when the ordered
path/type set is unchanged. If the imported structure itself changes, obsolete
dynamic outputs—and therefore their links—are intentionally removed rather
than silently pointing at a different field.

The compact metadata snapshot is saved with the workflow, so a browser-chosen
file need not remain available later. Editing `source` makes a mismatched
snapshot invalid; inspect again to rebuild it.

## Run PowerShell

1. Connect terminal/passthrough outputs to the autogrowing `when_*` sockets.
2. Paste the PowerShell source into `script`.
3. Optionally set **temp value count** from `0` through `32`.
4. Queue the workflow.

ComfyUI resolves every connected input before calling the node. Four connected
save outputs therefore form a four-way completion barrier. The node is also an
output node, so it can be queued without any connected triggers; an unconnected
`when_*` socket does not block it. “All nodes completed” can only be proven for
branches whose terminal outputs are connected—unrelated graph branches have no
data dependency and are not secretly synchronized.

The count slider grows `$temp1`, `$temp2`, and so on as integer inputs. A socket
can receive an upstream `INT`, or its adjacent manual widget is used as the
fallback. At `0`, no temporary-value inputs exist. Before the user source, the
node safely emits the equivalent of:

```powershell
$temp1 = 8192
$temp2 = 4096
```

Only validated integers enter that prelude. Variables live solely in this one
process and disappear on exit.

```powershell
$files = @(
    'D:\ComfyUI\output\tile-1.png'
    'D:\ComfyUI\output\tile-2.png'
)

$message = @'
All upstream outputs completed.
Starting organization pass.
'@

Write-Output "$message ($temp1 x $temp2)"
$files | ForEach-Object { Write-Output "Ready: `"$_`"" }
```

Each execution writes the user source unchanged to a uniquely named temporary
`.ps1`. When `$temp*` values exist, a second tiny temporary bootstrap defines
them and dot-sources the untouched user file. It then runs
`-NoProfile -NonInteractive`, captures stdout/stderr and the exit code, and
deletes every temporary file. A nonzero exit is displayed without crashing the
workflow. On timeout or ComfyUI interruption, the process tree is terminated.

Turning **enabled** off performs a clean no-op after its dependencies resolve.
Putting the node in ComfyUI **Bypass** mode is stronger: ComfyUI omits bypassed
nodes from the execution prompt, so ComfyShell's backend is never called and no
shell can start.

The script inherits the filesystem and process permissions of ComfyUI. That is
intentional: it can move or delete anything the ComfyUI process can access.

Core **Save Image** has no output socket. To enforce post-save ordering, use a
save node with a genuine post-save/passthrough output. Connecting the pre-save
image to both the save node and ComfyShell creates parallel branches and does
not prove the save finished.

Current ComfyUI limitation: Autogrow inputs do not propagate reliably through
subgraph boundaries, so keep the completion barrier in the containing graph.

## Verify

From the repository:

```powershell
python -m unittest discover -s tests -v
python -m py_compile metadata_importer.py powershell_runner.py
node tests/test_frontend.cjs
```

The tests cover API and UI workflows, metadata-only PNG inspection, latent
dimension inference, stale snapshots, multiline/here-string fidelity, nonzero
exits, timeouts, working directories, integer injection, and fresh-process
isolation. The small mocked frontend test verifies typed-output rebuilding and
the `0…N…0` temporary-input lifecycle without launching a browser.
