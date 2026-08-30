"""Extract reusable scalar values from ComfyUI workflows and PNG metadata.

The parser deliberately reads PNG metadata chunks without decoding pixels. A
multi-gigapixel generation therefore costs kilobytes of I/O plus seeks rather
than enough RAM to materialize the image.
"""

from __future__ import annotations

from collections import OrderedDict, deque
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import struct
from typing import Any, Iterable, Mapping
import zlib


MAX_OUTPUT_VALUES = 512
SNAPSHOT_VERSION = 1
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
PNG_TEXT_CHUNKS = {b"tEXt", b"zTXt", b"iTXt"}
MAX_PNG_TEXT_BYTES = 64 * 1024 * 1024


@dataclass(frozen=True)
class ParsedSource:
    payload: Any
    source_name: str
    source_input: str
    image_width: int | None = None
    image_height: int | None = None


def _json_load_maybe(value: Any) -> Any:
    if not isinstance(value, str):
        return value
    stripped = value.lstrip("\ufeff \t\r\n")
    if not stripped or stripped[0] not in "[{":
        return value
    try:
        return json.loads(stripped)
    except json.JSONDecodeError:
        return value


def _split_null(data: bytes) -> tuple[bytes, bytes]:
    head, separator, tail = data.partition(b"\0")
    if not separator:
        raise ValueError("malformed PNG text chunk")
    return head, tail


def _decode_png_text(chunk_type: bytes, data: bytes) -> tuple[str, str]:
    keyword_bytes, rest = _split_null(data)
    keyword = keyword_bytes.decode("latin-1")
    if chunk_type == b"tEXt":
        return keyword, rest.decode("latin-1")
    if chunk_type == b"zTXt":
        if not rest or rest[0] != 0:
            raise ValueError("unsupported PNG zTXt compression method")
        return keyword, zlib.decompress(rest[1:]).decode("latin-1")

    # iTXt: compression flag, compression method, language tag, translated
    # keyword, then UTF-8 text (optionally deflated).
    if len(rest) < 2:
        raise ValueError("malformed PNG iTXt chunk")
    compressed, method = rest[0], rest[1]
    if compressed not in (0, 1) or method != 0:
        raise ValueError("unsupported PNG iTXt compression settings")
    _language, rest = _split_null(rest[2:])
    _translated, text = _split_null(rest)
    if compressed:
        text = zlib.decompress(text)
    return keyword, text.decode("utf-8")


def read_png_metadata(path: Path) -> tuple[dict[str, str], int | None, int | None]:
    """Read textual metadata and IHDR dimensions without decoding PNG pixels."""

    metadata: dict[str, str] = {}
    width: int | None = None
    height: int | None = None
    text_bytes = 0

    with path.open("rb") as handle:
        if handle.read(8) != PNG_SIGNATURE:
            raise ValueError(f"not a PNG file: {path}")
        while True:
            length_bytes = handle.read(4)
            if not length_bytes:
                break
            if len(length_bytes) != 4:
                raise ValueError("truncated PNG chunk length")
            length = struct.unpack(">I", length_bytes)[0]
            chunk_type = handle.read(4)
            if len(chunk_type) != 4:
                raise ValueError("truncated PNG chunk type")

            if chunk_type == b"IHDR":
                data = handle.read(length)
                if len(data) != length or length < 8:
                    raise ValueError("truncated PNG IHDR")
                width, height = struct.unpack(">II", data[:8])
            elif chunk_type in PNG_TEXT_CHUNKS:
                text_bytes += length
                if text_bytes > MAX_PNG_TEXT_BYTES:
                    raise ValueError("PNG textual metadata exceeds 64 MiB safety limit")
                data = handle.read(length)
                if len(data) != length:
                    raise ValueError("truncated PNG text chunk")
                try:
                    key, value = _decode_png_text(chunk_type, data)
                except (UnicodeError, zlib.error) as error:
                    readable_type = chunk_type.decode("ascii", errors="replace")
                    raise ValueError(f"invalid PNG {readable_type} metadata") from error
                metadata[key] = value
            else:
                handle.seek(length, os.SEEK_CUR)

            if len(handle.read(4)) != 4:
                raise ValueError("truncated PNG chunk CRC")
            if chunk_type == b"IEND":
                break

    return metadata, width, height


def parse_source(source: str) -> ParsedSource:
    source_input = source.strip()
    if not source_input:
        raise ValueError("provide JSON text or a .json/.png file path")

    expanded = Path(os.path.expandvars(os.path.expanduser(source_input)))
    if expanded.is_file():
        path = expanded.resolve()
        if path.suffix.lower() == ".png":
            metadata, width, height = read_png_metadata(path)
            return ParsedSource(
                payload={"metadata": metadata},
                source_name=path.name,
                source_input=source_input,
                image_width=width,
                image_height=height,
            )
        try:
            payload = json.loads(path.read_text(encoding="utf-8-sig"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValueError(f"file is not valid UTF-8 JSON: {path}") from error
        return ParsedSource(payload, path.name, source_input)

    try:
        payload = json.loads(source_input)
    except json.JSONDecodeError as error:
        raise ValueError(
            "source is neither an existing file path nor valid JSON: "
            f"{error.msg} at line {error.lineno}, column {error.colno}"
        ) from error
    return ParsedSource(payload, "pasted JSON", source_input)


def parsed_source_from_browser_payload(body: Mapping[str, Any]) -> ParsedSource:
    """Convert the browser route request into the same canonical source type."""

    if isinstance(body.get("source"), str) and body["source"].strip():
        return parse_source(body["source"])
    payload = body.get("payload")
    if payload is None:
        raise ValueError("inspection request has neither source nor payload")
    width = body.get("image_width")
    height = body.get("image_height")
    return ParsedSource(
        payload=payload,
        source_name=str(body.get("source_name") or "browser selection"),
        source_input=str(body.get("source_input") or body.get("source_name") or ""),
        image_width=int(width) if isinstance(width, (int, float)) else None,
        image_height=int(height) if isinstance(height, (int, float)) else None,
    )


def _stable_node_key(value: Any) -> tuple[int, int | str]:
    text = str(value)
    return (0, int(text)) if text.isdigit() else (1, text.casefold())


def _ui_order(node: Mapping[str, Any]) -> int:
    order = node.get("order")
    return order if isinstance(order, int) else 2**31


def _is_scalar(value: Any) -> bool:
    return value is None or isinstance(value, (bool, int, float, str))


def _value_type(value: Any) -> str:
    if isinstance(value, bool):
        return "BOOLEAN"
    if isinstance(value, int):
        return "INT"
    if isinstance(value, float):
        return "FLOAT"
    return "STRING"


def _normalized_scalar(value: Any) -> bool | int | float | str:
    return "null" if value is None else value


def _flatten_values(value: Any, prefix: str = "") -> Iterable[tuple[str, Any]]:
    if _is_scalar(value):
        yield prefix, _normalized_scalar(value)
        return
    if isinstance(value, Mapping):
        for key, child in value.items():
            child_prefix = f"{prefix}.{key}" if prefix else str(key)
            yield from _flatten_values(child, child_prefix)
        return
    if isinstance(value, (list, tuple)):
        for index, child in enumerate(value):
            child_prefix = f"{prefix}.{index}" if prefix else str(index)
            yield from _flatten_values(child, child_prefix)
        return
    yield prefix, str(value)


def _short_label(value: str, limit: int = 88) -> str:
    compact = " ".join(value.split())
    if len(compact) <= limit:
        return compact
    return f"{compact[: limit - 1]}…"


def _descriptor(path: str, label: str, value: Any) -> dict[str, Any]:
    normalized = _normalized_scalar(value)
    return {
        "path": path,
        "label": _short_label(label),
        "type": _value_type(normalized),
        "value": normalized,
    }


def _looks_like_api_prompt(value: Any) -> bool:
    if not isinstance(value, Mapping) or not value:
        return False
    found_node = False
    for key, node in value.items():
        if isinstance(key, str) and key.startswith("_"):
            continue
        if not (
            isinstance(node, Mapping)
            and isinstance(node.get("class_type"), str)
            and isinstance(node.get("inputs", {}), Mapping)
        ):
            return False
        found_node = True
    return found_node


def _api_nodes(prompt: Mapping[str, Any]) -> dict[str, Mapping[str, Any]]:
    return {
        str(node_id): node
        for node_id, node in prompt.items()
        if isinstance(node, Mapping)
        and isinstance(node.get("class_type"), str)
        and isinstance(node.get("inputs", {}), Mapping)
    }


def _looks_like_ui_workflow(value: Any) -> bool:
    return isinstance(value, Mapping) and isinstance(value.get("nodes"), list)


def _unwrap_payload(payload: Any) -> tuple[Any | None, Any | None, str | None]:
    api_prompt: Any | None = None
    ui_workflow: Any | None = None
    parameters: str | None = None

    if isinstance(payload, Mapping) and isinstance(payload.get("metadata"), Mapping):
        metadata = payload["metadata"]
        prompt = _json_load_maybe(metadata.get("prompt"))
        workflow = _json_load_maybe(metadata.get("workflow"))
        parameters_value = metadata.get("parameters")
        if isinstance(parameters_value, str):
            parameters = parameters_value
        if _looks_like_api_prompt(prompt):
            api_prompt = prompt
        elif _looks_like_ui_workflow(prompt):
            ui_workflow = prompt
        if _looks_like_ui_workflow(workflow):
            ui_workflow = workflow
        elif _looks_like_api_prompt(workflow):
            api_prompt = workflow

    if isinstance(payload, Mapping):
        prompt = _json_load_maybe(payload.get("prompt"))
        output = _json_load_maybe(payload.get("output"))
        explicit_api = _json_load_maybe(payload.get("api_prompt"))
        workflow = _json_load_maybe(payload.get("workflow"))
        if _looks_like_api_prompt(prompt):
            api_prompt = prompt
        elif _looks_like_ui_workflow(prompt):
            ui_workflow = prompt
        if _looks_like_api_prompt(output):
            api_prompt = output
        if _looks_like_api_prompt(explicit_api):
            api_prompt = explicit_api
        if _looks_like_ui_workflow(workflow):
            ui_workflow = workflow
        elif _looks_like_api_prompt(workflow):
            api_prompt = workflow
        if isinstance(payload.get("parameters"), str):
            parameters = payload["parameters"]

    if _looks_like_api_prompt(payload):
        api_prompt = payload
    elif _looks_like_ui_workflow(payload):
        ui_workflow = payload
    return api_prompt, ui_workflow, parameters


def _ui_node_info(workflow: Any) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    if not _looks_like_ui_workflow(workflow):
        return result
    for node in workflow["nodes"]:
        if not isinstance(node, Mapping) or "id" not in node:
            continue
        node_id = str(node["id"])
        title = node.get("title") or node.get("type") or f"node {node_id}"
        result[node_id] = {
            "node": node,
            "title": str(title),
            "type": str(node.get("type") or "Unknown"),
            "order": node.get("order"),
        }
    return result


def _api_link(value: Any, node_ids: set[str]) -> tuple[str, int] | None:
    if (
        isinstance(value, list)
        and len(value) == 2
        and str(value[0]) in node_ids
        and isinstance(value[1], int)
    ):
        return str(value[0]), value[1]
    return None


def _node_label(node_id: str, class_type: str, ui_info: Mapping[str, Any]) -> str:
    title = ui_info.get(node_id, {}).get("title")
    if title and str(title) != class_type:
        return f"#{node_id} {title} ({class_type})"
    return f"#{node_id} {class_type}"


def _extract_api_values(
    prompt: Mapping[str, Any], ui_info: Mapping[str, Any]
) -> OrderedDict[str, dict[str, Any]]:
    values: OrderedDict[str, dict[str, Any]] = OrderedDict()
    nodes = _api_nodes(prompt)
    node_ids = set(nodes)
    for node_id in sorted(nodes, key=_stable_node_key):
        node = nodes[node_id]
        class_type = str(node.get("class_type") or "Unknown")
        label = _node_label(node_id, class_type, ui_info)
        inputs = node.get("inputs", {})
        for input_name, value in inputs.items():
            if _api_link(value, node_ids) is not None:
                continue
            for suffix, scalar in _flatten_values(value):
                field = str(input_name) + (f".{suffix}" if suffix else "")
                path = f"node.{node_id}.{field}"
                values[path] = _descriptor(path, f"{label} · {field}", scalar)
    return values


def _named_ui_widgets(node: Mapping[str, Any]) -> OrderedDict[str, Any]:
    named = node.get("widgets_values_named")
    if isinstance(named, Mapping):
        return OrderedDict((str(key), value) for key, value in named.items())

    positional = node.get("widgets_values")
    if not isinstance(positional, list):
        return OrderedDict()
    names: list[str] = []
    for input_spec in node.get("inputs", []) or []:
        if not isinstance(input_spec, Mapping):
            continue
        widget = input_spec.get("widget")
        if isinstance(widget, Mapping) and isinstance(widget.get("name"), str):
            names.append(widget["name"])
    result: OrderedDict[str, Any] = OrderedDict()
    for index, value in enumerate(positional):
        name = names[index] if index < len(names) else f"widget_{index}"
        result[name] = value
    return result


def _extract_ui_values(
    workflow: Mapping[str, Any], existing: Mapping[str, Any]
) -> OrderedDict[str, dict[str, Any]]:
    values: OrderedDict[str, dict[str, Any]] = OrderedDict()
    nodes = [node for node in workflow.get("nodes", []) if isinstance(node, Mapping)]
    nodes.sort(key=lambda node: (_ui_order(node), _stable_node_key(node.get("id", ""))))
    for node in nodes:
        node_id = str(node.get("id", "?"))
        node_type = str(node.get("type") or "Unknown")
        title = str(node.get("title") or node_type)
        base_label = f"#{node_id} {title}"
        if title != node_type:
            base_label += f" ({node_type})"
        for widget_name, value in _named_ui_widgets(node).items():
            for suffix, scalar in _flatten_values(value):
                field = widget_name + (f".{suffix}" if suffix else "")
                path = f"node.{node_id}.{field}"
                if path in existing:
                    continue
                values[path] = _descriptor(path, f"{base_label} · {field}", scalar)
    return values


def _workflow_link_origins(workflow: Mapping[str, Any]) -> dict[int, str]:
    origins: dict[int, str] = {}
    for link in workflow.get("links", []) or []:
        if isinstance(link, list) and len(link) >= 3 and isinstance(link[0], int):
            origins[link[0]] = str(link[1])
    return origins


def _ui_upstream_node(
    node: Mapping[str, Any], input_names: tuple[str, ...], origins: Mapping[int, str]
) -> str | None:
    for spec in node.get("inputs", []) or []:
        if not isinstance(spec, Mapping) or spec.get("name") not in input_names:
            continue
        link_id = spec.get("link")
        if isinstance(link_id, int) and link_id in origins:
            return origins[link_id]
    return None


def _scalar_int(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, float) and value.is_integer():
        return int(value)
    if isinstance(value, str) and value.strip().lstrip("-").isdigit():
        return int(value)
    return None


def _api_resolve_int(
    value: Any,
    by_id: Mapping[str, Mapping[str, Any]],
    node_ids: set[str],
    visited: set[str] | None = None,
) -> int | None:
    direct = _scalar_int(value)
    if direct is not None:
        return direct
    link = _api_link(value, node_ids)
    if link is None:
        return None
    node_id = link[0]
    visited = set() if visited is None else visited
    if node_id in visited or node_id not in by_id:
        return None
    visited.add(node_id)
    inputs = by_id[node_id].get("inputs", {})
    for name in ("value", "int", "number", "width", "height"):
        if name in inputs:
            resolved = _api_resolve_int(inputs[name], by_id, node_ids, visited)
            if resolved is not None:
                return resolved
    candidates = [
        resolved
        for child in inputs.values()
        if (resolved := _api_resolve_int(child, by_id, node_ids, visited.copy())) is not None
    ]
    return candidates[0] if len(candidates) == 1 else None


def _api_latent_dimensions(prompt: Mapping[str, Any], sampler_id: str) -> tuple[int, int] | None:
    by_id = _api_nodes(prompt)
    node_ids = set(by_id)
    sampler = by_id[sampler_id]
    start = None
    for name in ("latent_image", "samples", "latent", "image"):
        link = _api_link(sampler.get("inputs", {}).get(name), node_ids)
        if link:
            start = link[0]
            break
    if start is None:
        return None

    queue = deque([start])
    visited: set[str] = set()
    while queue:
        node_id = queue.popleft()
        if node_id in visited or node_id not in by_id:
            continue
        visited.add(node_id)
        inputs = by_id[node_id].get("inputs", {})
        width = _api_resolve_int(inputs.get("width"), by_id, node_ids)
        height = _api_resolve_int(inputs.get("height"), by_id, node_ids)
        if width is not None and height is not None:
            return width, height
        linked: list[str] = []
        for name, value in inputs.items():
            link = _api_link(value, node_ids)
            if link:
                priority = 0 if name in ("latent_image", "samples", "latent", "image") else 1
                linked.append(f"{priority}:{link[0]}")
        for encoded in sorted(linked):
            queue.append(encoded.split(":", 1)[1])
    return None


def _ui_latent_dimensions(workflow: Mapping[str, Any], sampler: Mapping[str, Any]) -> tuple[int, int] | None:
    by_id = {
        str(node.get("id")): node
        for node in workflow.get("nodes", [])
        if isinstance(node, Mapping) and "id" in node
    }
    origins = _workflow_link_origins(workflow)
    start = _ui_upstream_node(sampler, ("latent_image", "samples", "latent", "image"), origins)
    if start is None:
        return None
    queue = deque([start])
    visited: set[str] = set()

    def linked_int(node: Mapping[str, Any], field: str) -> int | None:
        direct = _scalar_int(_named_ui_widgets(node).get(field))
        if direct is not None:
            return direct
        origin = _ui_upstream_node(node, (field,), origins)
        if origin is None:
            return None
        local_seen: set[str] = set()
        pending = deque([origin])
        while pending:
            origin_id = pending.popleft()
            if origin_id in local_seen or origin_id not in by_id:
                continue
            local_seen.add(origin_id)
            origin_node = by_id[origin_id]
            widgets = _named_ui_widgets(origin_node)
            for name in ("value", "int", "number", field):
                resolved = _scalar_int(widgets.get(name))
                if resolved is not None:
                    return resolved
            scalar_widgets = [
                resolved
                for value in widgets.values()
                if (resolved := _scalar_int(value)) is not None
            ]
            if len(scalar_widgets) == 1:
                return scalar_widgets[0]
            for spec in origin_node.get("inputs", []) or []:
                if not isinstance(spec, Mapping):
                    continue
                link_id = spec.get("link")
                if isinstance(link_id, int) and link_id in origins:
                    pending.append(origins[link_id])
        return None

    while queue:
        node_id = queue.popleft()
        if node_id in visited or node_id not in by_id:
            continue
        visited.add(node_id)
        node = by_id[node_id]
        width = linked_int(node, "width")
        height = linked_int(node, "height")
        if width is not None and height is not None:
            return width, height
        for spec in node.get("inputs", []) or []:
            if not isinstance(spec, Mapping):
                continue
            link_id = spec.get("link")
            if isinstance(link_id, int) and link_id in origins:
                queue.append(origins[link_id])
    return None


def _inferred_values(api_prompt: Any, ui_workflow: Any) -> list[dict[str, Any]]:
    inferred: list[dict[str, Any]] = []
    if _looks_like_api_prompt(api_prompt):
        ui_info = _ui_node_info(ui_workflow)
        candidates: list[tuple[Any, str, Mapping[str, Any]]] = []
        for raw_id, node in _api_nodes(api_prompt).items():
            class_type = str(node.get("class_type") or "")
            input_names = set(node.get("inputs", {}))
            if "sampler" not in class_type.casefold() or not input_names.intersection(
                {"latent_image", "samples", "latent"}
            ):
                continue
            info = ui_info.get(str(raw_id), {})
            order = info.get("order")
            candidates.append((order if isinstance(order, int) else 2**31, str(raw_id), node))
        if candidates:
            _order, sampler_id, sampler = min(
                candidates, key=lambda item: (item[0], _stable_node_key(item[1]))
            )
            class_type = str(sampler.get("class_type") or "Unknown")
            inferred.append(_descriptor("inferred.sampler.node_id", "inferred · first sampler node", sampler_id))
            inferred.append(_descriptor("inferred.sampler.class_type", "inferred · first sampler type", class_type))
            dimensions = _api_latent_dimensions(api_prompt, sampler_id)
            if dimensions:
                inferred.append(_descriptor("inferred.latent.width", "inferred · initial latent width", dimensions[0]))
                inferred.append(_descriptor("inferred.latent.height", "inferred · initial latent height", dimensions[1]))
            return inferred

    if _looks_like_ui_workflow(ui_workflow):
        candidates = [
            node
            for node in ui_workflow.get("nodes", [])
            if isinstance(node, Mapping)
            and "sampler" in str(node.get("type", "")).casefold()
            and {
                str(spec.get("name"))
                for spec in node.get("inputs", []) or []
                if isinstance(spec, Mapping)
            }.intersection({"latent_image", "samples", "latent"})
        ]
        candidates.sort(key=lambda node: (_ui_order(node), _stable_node_key(node.get("id", ""))))
        if candidates:
            sampler = candidates[0]
            sampler_id = str(sampler.get("id", "?"))
            inferred.append(_descriptor("inferred.sampler.node_id", "inferred · first sampler node", sampler_id))
            inferred.append(_descriptor("inferred.sampler.class_type", "inferred · first sampler type", str(sampler.get("type") or "Unknown")))
            dimensions = _ui_latent_dimensions(ui_workflow, sampler)
            if dimensions:
                inferred.append(_descriptor("inferred.latent.width", "inferred · initial latent width", dimensions[0]))
                inferred.append(_descriptor("inferred.latent.height", "inferred · initial latent height", dimensions[1]))
    return inferred


PARAMETER_PAIR = re.compile(r"(?:^|,\s*)([^:,]+):\s*([^,]+)")


def _parameter_values(parameters: str | None) -> list[dict[str, Any]]:
    if not parameters:
        return []
    lines = parameters.replace("\r\n", "\n").split("\n")
    values: list[dict[str, Any]] = []
    if lines and lines[0].strip():
        values.append(_descriptor("parameters.prompt", "parameters · positive prompt", lines[0]))
    negative = next((line[16:] for line in lines if line.startswith("Negative prompt: ")), None)
    if negative is not None:
        values.append(_descriptor("parameters.negative_prompt", "parameters · negative prompt", negative))
    settings = lines[-1] if lines else ""
    for match in PARAMETER_PAIR.finditer(settings):
        key = match.group(1).strip()
        raw = match.group(2).strip()
        value: Any = raw
        if raw.lstrip("-").isdigit():
            value = int(raw)
        else:
            try:
                value = float(raw)
            except ValueError:
                pass
        path_key = re.sub(r"[^a-z0-9]+", "_", key.casefold()).strip("_")
        values.append(_descriptor(f"parameters.{path_key}", f"parameters · {key}", value))
    return values


def build_snapshot(parsed: ParsedSource) -> dict[str, Any]:
    api_prompt, ui_workflow, parameters = _unwrap_payload(parsed.payload)
    if api_prompt is None and ui_workflow is None and parameters is None:
        raise ValueError("no ComfyUI prompt/workflow metadata was found")

    ui_info = _ui_node_info(ui_workflow)
    node_values: OrderedDict[str, dict[str, Any]] = OrderedDict()
    if _looks_like_api_prompt(api_prompt):
        node_values.update(_extract_api_values(api_prompt, ui_info))
    if _looks_like_ui_workflow(ui_workflow):
        node_values.update(_extract_ui_values(ui_workflow, node_values))

    values: list[dict[str, Any]] = []
    if parsed.image_width is not None and parsed.image_height is not None:
        values.extend(
            [
                _descriptor("source.image.width", "source image · width", parsed.image_width),
                _descriptor("source.image.height", "source image · height", parsed.image_height),
            ]
        )
    values.extend(_inferred_values(api_prompt, ui_workflow))
    values.extend(_parameter_values(parameters))

    seen = {item["path"] for item in values}
    values.extend(item for path, item in node_values.items() if path not in seen)
    total = len(values)
    values = values[:MAX_OUTPUT_VALUES]
    fingerprint = hashlib.sha256(parsed.source_input.encode("utf-8")).hexdigest()
    return {
        "version": SNAPSHOT_VERSION,
        "source_name": parsed.source_name,
        "source_input": parsed.source_input,
        "source_fingerprint": fingerprint,
        "count": len(values),
        "total_count": total,
        "truncated": total > len(values),
        "values": values,
    }


def inspect_source(source: str) -> dict[str, Any]:
    return build_snapshot(parse_source(source))


def snapshot_from_json(snapshot_json: str, source: str = "") -> dict[str, Any] | None:
    if not snapshot_json.strip():
        return None
    try:
        snapshot = json.loads(snapshot_json)
    except json.JSONDecodeError:
        return None
    if not isinstance(snapshot, Mapping) or snapshot.get("version") != SNAPSHOT_VERSION:
        return None
    values = snapshot.get("values")
    if not isinstance(values, list):
        return None
    if source.strip():
        fingerprint = hashlib.sha256(source.strip().encode("utf-8")).hexdigest()
        if snapshot.get("source_fingerprint") != fingerprint:
            return None
    return dict(snapshot)


def snapshot_summary(snapshot: Mapping[str, Any]) -> str:
    count = int(snapshot.get("count", 0))
    total = int(snapshot.get("total_count", count))
    source_name = str(snapshot.get("source_name") or "workflow metadata")
    suffix = f"; showing first {count} of {total}" if total > count else ""
    return f"ComfyShell imported {count} typed value(s) from {source_name}{suffix}"


def snapshot_outputs(snapshot: Mapping[str, Any]) -> tuple[Any, ...]:
    values = snapshot.get("values", [])
    output_values = [item.get("value") for item in values if isinstance(item, Mapping)]
    output_values = output_values[:MAX_OUTPUT_VALUES]
    output_values.extend([None] * (MAX_OUTPUT_VALUES - len(output_values)))
    return (snapshot_summary(snapshot), *output_values)
