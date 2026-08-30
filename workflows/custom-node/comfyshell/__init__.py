"""ComfyShell: workflow metadata outputs and synchronized one-shot pwsh."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import time
from typing import Any

from aiohttp import web
from comfy_api.latest import ComfyExtension, io, ui
from server import PromptServer

from .metadata_importer import (
    MAX_OUTPUT_VALUES,
    build_snapshot,
    inspect_source,
    parsed_source_from_browser_payload,
    snapshot_from_json,
    snapshot_outputs,
    snapshot_summary,
)
from .powershell_runner import (
    MAX_TEMP_VALUES,
    collect_temp_variables,
    format_result,
    run_powershell,
)


WEB_DIRECTORY = "./web"


@PromptServer.instance.routes.post("/comfyshell/inspect")
async def inspect_metadata_route(request: web.Request) -> web.Response:
    """Build a compact typed snapshot for the frontend's dynamic outputs."""

    try:
        body = await request.json()
        if not isinstance(body, dict):
            raise ValueError("request body must be a JSON object")
        snapshot = build_snapshot(parsed_source_from_browser_payload(body))
        return web.json_response({"ok": True, "snapshot": snapshot})
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        return web.json_response({"ok": False, "error": str(error)}, status=400)


class ComfyShellImportWorkflowMetadata(io.ComfyNode):
    @classmethod
    def define_schema(cls) -> io.Schema:
        outputs: list[Any] = [io.String.Output("summary", display_name="summary")]
        outputs.extend(
            io.AnyType.Output(f"value_{index}", display_name=f"value_{index}")
            for index in range(MAX_OUTPUT_VALUES)
        )
        return io.Schema(
            node_id="ComfyShell_ImportWorkflowMetadata",
            display_name="Import Workflow Metadata",
            category="ComfyShell/Workflow",
            description=(
                "Reads pasted JSON, a .json path, or Comfy PNG metadata and exposes "
                "the stored settings as typed, connectable outputs. PNG pixels are never decoded."
            ),
            search_aliases=["workflow values", "PNG metadata", "seed importer"],
            inputs=[
                io.String.Input(
                    "source",
                    default="",
                    multiline=True,
                    tooltip=(
                        "Paste workflow/API JSON or enter a local .json/.png path. "
                        "The Load button can also inspect a browser-selected file."
                    ),
                ),
                io.String.Input(
                    "snapshot_json",
                    default="",
                    multiline=True,
                    advanced=True,
                    tooltip="Frontend-managed typed metadata snapshot; normally leave untouched.",
                ),
            ],
            outputs=outputs,
        )

    @classmethod
    def fingerprint_inputs(cls, source: str, snapshot_json: str, **_kwargs: Any) -> str:
        expanded = Path(os.path.expandvars(os.path.expanduser(source.strip())))
        try:
            stat = expanded.stat()
            file_state = f"{expanded.resolve()}:{stat.st_size}:{stat.st_mtime_ns}"
        except (OSError, ValueError):
            file_state = source
        return hashlib.sha256(f"{file_state}\0{snapshot_json}".encode("utf-8")).hexdigest()

    @classmethod
    def execute(cls, source: str, snapshot_json: str) -> io.NodeOutput:
        snapshot = snapshot_from_json(snapshot_json, source)
        if snapshot is None:
            snapshot = inspect_source(source)
        summary = snapshot_summary(snapshot)
        return io.NodeOutput(*snapshot_outputs(snapshot), ui=ui.PreviewText(summary))


class ComfyShellRunPowerShell(io.ComfyNode):
    @classmethod
    def define_schema(cls) -> io.Schema:
        triggers = io.Autogrow.TemplatePrefix(
            input=io.AnyType.Input("when"),
            prefix="when_",
            min=1,
            max=64,
        )
        return io.Schema(
            # Preserve the original ID so workflows made before the ComfyShell
            # rename continue to load without migration or replacement hooks.
            node_id="NativePowerShell_RunScript",
            display_name="Run PowerShell (ComfyShell)",
            category="ComfyShell/Automation",
            description=(
                "Waits for every connected 'when' input, then runs the script once "
                "inside a fresh, hidden, non-interactive pwsh process."
            ),
            search_aliases=["run script", "pwsh", "completion barrier"],
            is_output_node=True,
            not_idempotent=True,
            accept_all_inputs=True,
            inputs=[
                io.Autogrow.Input(
                    "when",
                    template=triggers,
                    tooltip=(
                        "Connect arbitrary terminal outputs. Execution starts only "
                        "after every connected dependency has completed."
                    ),
                ),
                io.String.Input(
                    "script",
                    default="Write-Output 'PowerShell workflow complete'",
                    multiline=True,
                    tooltip="PowerShell source; multiline text and here-strings are preserved verbatim.",
                ),
                io.Boolean.Input(
                    "enabled",
                    default=True,
                    tooltip="Off performs a clean no-op. ComfyUI bypass also prevents all execution.",
                ),
                io.Int.Input(
                    "temp_value_count",
                    display_name="number of value nodes",
                    default=0,
                    min=0,
                    max=MAX_TEMP_VALUES,
                    step=1,
                    display_mode=io.NumberDisplay.slider,
                    tooltip=(
                        "Adds integer temp1…tempN inputs. Zero exposes none. Values "
                        "become fresh $temp1…$tempN variables for this process only."
                    ),
                ),
                io.String.Input(
                    "temp_values_json",
                    default="{}",
                    multiline=False,
                    advanced=True,
                    tooltip="Frontend-managed fallback values; normally leave untouched.",
                ),
                io.String.Input(
                    "pwsh_executable",
                    default="pwsh",
                    advanced=True,
                    tooltip="Executable name from PATH, or an absolute path to pwsh.exe.",
                ),
                io.String.Input(
                    "working_directory",
                    default="",
                    advanced=True,
                    tooltip="Optional process working directory; empty inherits ComfyUI's directory.",
                ),
                io.Int.Input(
                    "timeout_seconds",
                    default=3600,
                    min=1,
                    max=86400,
                    step=1,
                    advanced=True,
                    tooltip="The PowerShell process tree is terminated when this limit is reached.",
                ),
            ],
            outputs=[io.String.Output("status", display_name="status")],
        )

    @classmethod
    def fingerprint_inputs(cls, **_kwargs: Any) -> int:
        # Side effects must occur once for every queued execution, even when
        # all graph inputs are served from ComfyUI's cache.
        return time.time_ns()

    @classmethod
    def execute(
        cls,
        when: io.Autogrow.Type,
        script: str,
        enabled: bool,
        temp_value_count: int,
        temp_values_json: str,
        pwsh_executable: str,
        working_directory: str,
        timeout_seconds: int,
        **dynamic_inputs: Any,
    ) -> io.NodeOutput:
        dependency_count = len(when)
        del when

        if not enabled:
            status = (
                f"PowerShell skipped (disabled) after {dependency_count} "
                "trigger input(s); no process was started"
            )
            print(f"[ComfyShell] {status}")
            return io.NodeOutput(status, ui=ui.PreviewText(status))

        try:
            variables = collect_temp_variables(
                temp_value_count,
                temp_values_json,
                dynamic_inputs,
            )
        except ValueError as error:
            status = f"PowerShell did not start: {error}"
            print(f"[ComfyShell] {status}")
            return io.NodeOutput(status, ui=ui.PreviewText(status))

        def check_interrupted() -> None:
            from comfy.model_management import throw_exception_if_processing_interrupted

            throw_exception_if_processing_interrupted()

        result = run_powershell(
            script,
            executable=pwsh_executable,
            working_directory=working_directory,
            timeout_seconds=timeout_seconds,
            interrupt_check=check_interrupted,
            variables=variables,
        )
        status = format_result(result, dependency_count)
        print(f"[ComfyShell] {status}")
        return io.NodeOutput(status, ui=ui.PreviewText(status))


class ComfyShellExtension(ComfyExtension):
    async def get_node_list(self) -> list[type[io.ComfyNode]]:
        return [ComfyShellImportWorkflowMetadata, ComfyShellRunPowerShell]


async def comfy_entrypoint() -> ComfyShellExtension:
    return ComfyShellExtension()


__all__ = ["WEB_DIRECTORY", "comfy_entrypoint"]
