"""One-shot, headless PowerShell process execution."""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
from typing import Any, Callable, Mapping


InterruptCheck = Callable[[], None]
MAX_TEMP_VALUES = 32


@dataclass(frozen=True)
class PowerShellResult:
    exit_code: int | None
    stdout: str
    stderr: str
    elapsed_seconds: float
    timed_out: bool = False
    start_error: str | None = None

    @property
    def succeeded(self) -> bool:
        return self.start_error is None and not self.timed_out and self.exit_code == 0


def _unquote_executable(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        return value[1:-1]
    return value


def _resolve_working_directory(value: str) -> str | None:
    value = value.strip()
    if not value:
        return None
    path = Path(os.path.expandvars(os.path.expanduser(value))).resolve()
    if not path.is_dir():
        raise ValueError(f"working directory does not exist: {path}")
    return str(path)


def collect_temp_variables(
    count: int,
    saved_values_json: str,
    supplied_values: Mapping[str, Any],
) -> dict[str, int]:
    """Resolve the slider-selected ``$tempN`` values safely.

    Connected dynamic inputs arrive in ``supplied_values``. Unconnected inputs
    use the frontend-persisted JSON fallback. Only real integers are accepted,
    making the generated assignment prelude immune to script injection.
    """

    selected_count = int(count)
    if not 0 <= selected_count <= MAX_TEMP_VALUES:
        raise ValueError(
            f"temp value count must be between 0 and {MAX_TEMP_VALUES}, got {selected_count}"
        )

    saved: dict[str, Any] = {}
    if saved_values_json.strip():
        try:
            decoded = json.loads(saved_values_json)
        except ValueError as error:
            raise ValueError("saved temp values are not valid JSON") from error
        if not isinstance(decoded, dict):
            raise ValueError("saved temp values must be a JSON object")
        saved = decoded

    variables: dict[str, int] = {}
    for index in range(1, selected_count + 1):
        name = f"temp{index}"
        value = supplied_values.get(name, saved.get(name, 0))
        if isinstance(value, bool) or not isinstance(value, int):
            raise ValueError(f"{name} must be an integer, got {type(value).__name__}")
        variables[name] = value
    return variables


def compose_bootstrap(script_path: Path, variables: Mapping[str, int]) -> str:
    """Create the tiny wrapper that gives an untouched script its variables."""

    assignments = "".join(f"${name} = {int(value)}\n" for name, value in variables.items())
    escaped_path = str(script_path).replace("'", "''")
    return f"{assignments}\n. '{escaped_path}'\n"


def _terminate_process_tree(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return

    if os.name == "nt":
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        try:
            subprocess.run(
                ["taskkill.exe", "/PID", str(process.pid), "/T", "/F"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
                creationflags=flags,
            )
        except (OSError, subprocess.SubprocessError):
            pass
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except (OSError, ProcessLookupError):
            pass

    if process.poll() is None:
        process.kill()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        pass


def run_powershell(
    script: str,
    *,
    executable: str = "pwsh",
    working_directory: str = "",
    timeout_seconds: int = 3600,
    interrupt_check: InterruptCheck | None = None,
    variables: Mapping[str, int] | None = None,
) -> PowerShellResult:
    """Run ``script`` once in a fresh PowerShell process.

    The script is written verbatim to a uniquely named temporary ``.ps1`` so
    multiline strings, backticks, quotes, and here-strings require no extra
    command-line escaping. The file is removed in ``finally``.
    """

    started = time.monotonic()
    process: subprocess.Popen[str] | None = None
    script_paths: list[Path] = []
    stdout = ""
    stderr = ""

    try:
        program = _unquote_executable(executable)
        if not program:
            raise ValueError("PowerShell executable is empty")
        cwd = _resolve_working_directory(working_directory)
        timeout = max(1, int(timeout_seconds))

        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="",
            suffix=".ps1",
            prefix="comfyshell-pwsh-",
            delete=False,
        ) as handle:
            handle.write(script)
            script_path = Path(handle.name)
            script_paths.append(script_path)

        entry_path = script_path
        if variables:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                newline="",
                suffix=".ps1",
                prefix="comfyshell-bootstrap-",
                delete=False,
            ) as handle:
                handle.write(compose_bootstrap(script_path, variables))
                entry_path = Path(handle.name)
                script_paths.append(entry_path)

        command = [
            program,
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(entry_path),
        ]
        popen_kwargs: dict[str, object] = {
            "cwd": cwd,
            "stdin": subprocess.DEVNULL,
            "stdout": subprocess.PIPE,
            "stderr": subprocess.PIPE,
            "text": True,
            "encoding": "utf-8",
            "errors": "replace",
        }
        if os.name == "nt":
            popen_kwargs["creationflags"] = (
                getattr(subprocess, "CREATE_NO_WINDOW", 0)
                | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            )
        else:
            popen_kwargs["start_new_session"] = True

        process = subprocess.Popen(command, **popen_kwargs)  # type: ignore[arg-type]
        deadline = time.monotonic() + timeout

        while True:
            if interrupt_check is not None:
                interrupt_check()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                _terminate_process_tree(process)
                stdout, stderr = process.communicate()
                return PowerShellResult(
                    exit_code=process.returncode,
                    stdout=stdout,
                    stderr=stderr,
                    elapsed_seconds=time.monotonic() - started,
                    timed_out=True,
                )
            try:
                stdout, stderr = process.communicate(timeout=min(0.25, remaining))
                break
            except subprocess.TimeoutExpired:
                continue

        return PowerShellResult(
            exit_code=process.returncode,
            stdout=stdout,
            stderr=stderr,
            elapsed_seconds=time.monotonic() - started,
        )
    except (OSError, ValueError) as error:
        if process is not None:
            _terminate_process_tree(process)
        return PowerShellResult(
            exit_code=process.returncode if process is not None else None,
            stdout=stdout,
            stderr=stderr,
            elapsed_seconds=time.monotonic() - started,
            start_error=str(error),
        )
    except BaseException:
        if process is not None:
            _terminate_process_tree(process)
        raise
    finally:
        if process is not None and process.poll() is None:
            _terminate_process_tree(process)
        for script_path in script_paths:
            try:
                script_path.unlink(missing_ok=True)
            except OSError:
                pass


def format_result(result: PowerShellResult, dependency_count: int, limit: int = 12_000) -> str:
    if result.start_error is not None:
        headline = f"PowerShell did not start: {result.start_error}"
    elif result.timed_out:
        headline = (
            f"PowerShell timed out after {result.elapsed_seconds:.2f}s; "
            "its process tree was terminated"
        )
    else:
        headline = (
            f"PowerShell completed after {dependency_count} trigger input(s): "
            f"exit {result.exit_code} in {result.elapsed_seconds:.2f}s"
        )

    sections = [headline]
    if result.stdout.strip():
        sections.append(f"STDOUT\n{result.stdout.rstrip()}")
    if result.stderr.strip():
        sections.append(f"STDERR\n{result.stderr.rstrip()}")
    rendered = "\n\n".join(sections)
    if len(rendered) <= limit:
        return rendered
    omitted = len(rendered) - limit
    return f"{rendered[:limit]}\n\n[preview truncated; {omitted} character(s) omitted]"
