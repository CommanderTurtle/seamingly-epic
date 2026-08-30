from __future__ import annotations

import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
import zlib


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))

from metadata_importer import (  # noqa: E402
    MAX_OUTPUT_VALUES,
    inspect_source,
    snapshot_from_json,
    snapshot_outputs,
)


def api_prompt() -> dict[str, object]:
    return {
        "_comment": "metadata keys are not executable nodes",
        "3": {
            "class_type": "KSampler",
            "inputs": {
                "seed": 42,
                "steps": 30,
                "cfg": 7.5,
                "sampler_name": "dpmpp_2m",
                "latent_image": ["5", 0],
                "model": ["4", 0],
            },
        },
        "4": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": {"ckpt_name": "model.safetensors"},
        },
        "5": {
            "class_type": "EmptyLatentImage",
            "inputs": {"width": 1024, "height": 768, "batch_size": 1},
        },
    }


def png_chunk(chunk_type: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(chunk_type)
    crc = zlib.crc32(data, crc) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + chunk_type + data + struct.pack(">I", crc)


class MetadataImporterTests(unittest.TestCase):
    def test_api_prompt_extracts_typed_values_and_infers_latent_size(self) -> None:
        snapshot = inspect_source(json.dumps(api_prompt()))
        values = {item["path"]: item for item in snapshot["values"]}
        self.assertEqual(values["inferred.latent.width"]["value"], 1024)
        self.assertEqual(values["inferred.latent.height"]["value"], 768)
        self.assertEqual(values["node.3.seed"]["type"], "INT")
        self.assertEqual(values["node.3.cfg"]["type"], "FLOAT")
        self.assertNotIn("node.3.model.0", values)

    def test_nested_lora_and_switch_values_remain_connectable(self) -> None:
        prompt = api_prompt()
        prompt["12"] = {
            "class_type": "LoraStack",
            "inputs": {
                "enabled": True,
                "loras": [
                    {"name": "detail.safetensors", "strength_model": 0.8},
                    {"name": "style.safetensors", "strength_model": 0.35},
                ],
            },
        }
        snapshot = inspect_source(json.dumps(prompt))
        values = {item["path"]: item for item in snapshot["values"]}
        self.assertEqual(values["node.12.enabled"]["type"], "BOOLEAN")
        self.assertEqual(values["node.12.loras.0.name"]["value"], "detail.safetensors")
        self.assertEqual(values["node.12.loras.1.strength_model"]["value"], 0.35)

    def test_modern_ui_workflow_uses_named_widget_values(self) -> None:
        workflow = {
            "nodes": [
                {
                    "id": 1,
                    "type": "EmptyLatentImage",
                    "order": 0,
                    "inputs": [],
                    "widgets_values_named": {"width": 640, "height": 896},
                },
                {
                    "id": 2,
                    "type": "KSampler",
                    "title": "Old portrait sampler",
                    "order": 1,
                    "inputs": [{"name": "latent_image", "type": "LATENT", "link": 7}],
                    "widgets_values_named": {"seed": 99, "steps": 25, "cfg": 4.5},
                },
            ],
            "links": [[7, 1, 0, 2, 0, "LATENT"]],
        }
        snapshot = inspect_source(json.dumps(workflow))
        values = {item["path"]: item["value"] for item in snapshot["values"]}
        self.assertEqual(values["inferred.sampler.node_id"], "2")
        self.assertEqual(values["inferred.latent.width"], 640)
        self.assertEqual(values["node.2.seed"], 99)

    def test_linked_primitive_dimensions_are_resolved(self) -> None:
        prompt = api_prompt()
        prompt["5"]["inputs"]["width"] = ["10", 0]
        prompt["5"]["inputs"]["height"] = ["11", 0]
        prompt["10"] = {"class_type": "PrimitiveInt", "inputs": {"value": 1536}}
        prompt["11"] = {"class_type": "PrimitiveInt", "inputs": {"value": 2048}}
        snapshot = inspect_source(json.dumps(prompt))
        values = {item["path"]: item["value"] for item in snapshot["values"]}
        self.assertEqual(values["inferred.latent.width"], 1536)
        self.assertEqual(values["inferred.latent.height"], 2048)

    def test_combined_api_export_wrapper_is_supported(self) -> None:
        wrapper = {
            "workflow": {
                "nodes": [
                    {
                        "id": 3,
                        "type": "KSampler",
                        "title": "Historical sampler",
                        "widgets_values_named": {"seed": 1},
                    }
                ]
            },
            "output": api_prompt(),
        }
        snapshot = inspect_source(json.dumps(wrapper))
        seed = next(item for item in snapshot["values"] if item["path"] == "node.3.seed")
        self.assertEqual(seed["value"], 42)
        self.assertIn("Historical sampler", seed["label"])

    def test_png_metadata_is_read_without_pixel_decode(self) -> None:
        prompt_text = json.dumps(api_prompt()).encode("latin-1")
        ihdr = struct.pack(">IIBBBBB", 8192, 4096, 8, 2, 0, 0, 0)
        png = (
            b"\x89PNG\r\n\x1a\n"
            + png_chunk(b"IHDR", ihdr)
            + png_chunk(b"tEXt", b"prompt\0" + prompt_text)
            + png_chunk(b"IEND", b"")
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory, "generation.png")
            path.write_bytes(png)
            snapshot = inspect_source(str(path))
        values = {item["path"]: item["value"] for item in snapshot["values"]}
        self.assertEqual(values["source.image.width"], 8192)
        self.assertEqual(values["source.image.height"], 4096)
        self.assertEqual(values["node.3.seed"], 42)

    def test_snapshot_is_rejected_after_source_changes(self) -> None:
        source = json.dumps(api_prompt())
        snapshot = inspect_source(source)
        encoded = json.dumps(snapshot)
        self.assertIsNotNone(snapshot_from_json(encoded, source))
        self.assertIsNone(snapshot_from_json(encoded, source.replace("42", "43", 1)))

    def test_unrelated_json_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "no ComfyUI"):
            inspect_source('{"hello": "world"}')

    def test_output_bank_is_stable_and_reports_pathological_truncation(self) -> None:
        workflow = {
            "nodes": [
                {
                    "id": 1,
                    "type": "SyntheticSettings",
                    "order": 0,
                    "inputs": [],
                    "widgets_values_named": {
                        f"value_{index}": index for index in range(MAX_OUTPUT_VALUES + 5)
                    },
                }
            ]
        }
        snapshot = inspect_source(json.dumps(workflow))
        self.assertTrue(snapshot["truncated"])
        self.assertEqual(snapshot["count"], MAX_OUTPUT_VALUES)
        self.assertEqual(snapshot["total_count"], MAX_OUTPUT_VALUES + 5)
        outputs = snapshot_outputs(snapshot)
        self.assertEqual(len(outputs), MAX_OUTPUT_VALUES + 1)
        self.assertIn("showing first", outputs[0])


if __name__ == "__main__":
    unittest.main()
