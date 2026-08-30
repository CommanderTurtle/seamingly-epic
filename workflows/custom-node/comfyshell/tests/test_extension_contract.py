from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
from types import ModuleType, SimpleNamespace
import unittest


PROJECT_ROOT = Path(__file__).resolve().parents[1]


class Definition:
    def __init__(self, *args, **kwargs):
        self.args = args
        self.id = args[0] if args else kwargs.get("id")
        for key, value in kwargs.items():
            setattr(self, key, value)


class TypeWithInputOutput:
    Input = Definition
    Output = Definition


class Autogrow:
    Type = dict

    class TemplatePrefix(Definition):
        pass

    class Input(Definition):
        pass


class Schema(Definition):
    pass


class NodeOutput:
    def __init__(self, *values, ui=None):
        self.values = values
        self.ui = ui


class Routes:
    def __init__(self):
        self.paths: list[str] = []

    def post(self, path: str):
        self.paths.append(path)

        def decorator(function):
            return function

        return decorator


def load_extension_module():
    comfy_api = ModuleType("comfy_api")
    latest = ModuleType("comfy_api.latest")
    io = SimpleNamespace(
        ComfyNode=object,
        Schema=Schema,
        NodeOutput=NodeOutput,
        String=TypeWithInputOutput,
        AnyType=TypeWithInputOutput,
        Boolean=TypeWithInputOutput,
        Int=TypeWithInputOutput,
        Autogrow=Autogrow,
        NumberDisplay=SimpleNamespace(slider="slider"),
    )
    latest.ComfyExtension = object
    latest.io = io
    latest.ui = SimpleNamespace(PreviewText=lambda value: value)
    comfy_api.latest = latest
    sys.modules["comfy_api"] = comfy_api
    sys.modules["comfy_api.latest"] = latest

    routes = Routes()
    server = ModuleType("server")
    server.PromptServer = SimpleNamespace(instance=SimpleNamespace(routes=routes))
    sys.modules["server"] = server

    aiohttp = ModuleType("aiohttp")
    aiohttp.web = SimpleNamespace(
        Request=object,
        Response=object,
        json_response=lambda payload, status=200: (payload, status),
    )
    sys.modules["aiohttp"] = aiohttp

    name = "comfyshell_contract_test"
    spec = importlib.util.spec_from_file_location(
        name,
        PROJECT_ROOT / "__init__.py",
        submodule_search_locations=[str(PROJECT_ROOT)],
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module, routes


class ExtensionContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.extension, cls.routes = load_extension_module()

    def test_route_and_web_directory_are_registered(self) -> None:
        self.assertEqual(self.extension.WEB_DIRECTORY, "./web")
        self.assertIn("/comfyshell/inspect", self.routes.paths)

    def test_metadata_schema_has_stable_output_bank(self) -> None:
        schema = self.extension.ComfyShellImportWorkflowMetadata.define_schema()
        self.assertEqual(schema.node_id, "ComfyShell_ImportWorkflowMetadata")
        self.assertEqual(len(schema.outputs), self.extension.MAX_OUTPUT_VALUES + 1)

    def test_powershell_schema_preserves_old_id_and_accepts_dynamic_inputs(self) -> None:
        schema = self.extension.ComfyShellRunPowerShell.define_schema()
        self.assertEqual(schema.node_id, "NativePowerShell_RunScript")
        self.assertTrue(schema.accept_all_inputs)
        count = next(item for item in schema.inputs if item.id == "temp_value_count")
        self.assertEqual(count.min, 0)
        self.assertEqual(count.max, self.extension.MAX_TEMP_VALUES)

    def test_disabled_execution_is_a_noop_result(self) -> None:
        output = self.extension.ComfyShellRunPowerShell.execute(
            when={},
            script="throw 'must not run'",
            enabled=False,
            temp_value_count=0,
            temp_values_json="{}",
            pwsh_executable="missing-on-purpose",
            working_directory="",
            timeout_seconds=1,
        )
        self.assertIn("skipped (disabled)", output.values[0])


if __name__ == "__main__":
    unittest.main()
