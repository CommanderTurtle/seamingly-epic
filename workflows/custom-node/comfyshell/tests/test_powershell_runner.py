from __future__ import annotations

import sys
import tempfile
from pathlib import Path
import unittest


PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))

from powershell_runner import (  # noqa: E402
    collect_temp_variables,
    run_powershell,
)


class PowerShellRunnerTests(unittest.TestCase):
    def test_multiline_here_string_and_backtick_are_preserved(self) -> None:
        script = """$value = @'
alpha
beta
'@
Write-Output $value
Write-Output "grave`ntick"
"""
        result = run_powershell(script, timeout_seconds=10)
        self.assertTrue(result.succeeded, result)
        self.assertIn("alpha\nbeta", result.stdout.replace("\r\n", "\n"))
        self.assertIn("grave\ntick", result.stdout.replace("\r\n", "\n"))

    def test_nonzero_exit_is_a_result_not_an_exception(self) -> None:
        result = run_powershell(
            "[Console]::Error.WriteLine('expected failure')\nexit 7",
            timeout_seconds=10,
        )
        self.assertEqual(result.exit_code, 7)
        self.assertFalse(result.timed_out)
        self.assertIn("expected failure", result.stderr)

    def test_each_run_has_fresh_session_state(self) -> None:
        first = run_powershell(
            "$global:ComfyNodeProbe = 'present'\nWrite-Output $global:ComfyNodeProbe",
            timeout_seconds=10,
        )
        second = run_powershell(
            "if (Get-Variable ComfyNodeProbe -Scope Global -ErrorAction SilentlyContinue) "
            "{ exit 9 } else { Write-Output 'isolated' }",
            timeout_seconds=10,
        )
        self.assertTrue(first.succeeded, first)
        self.assertTrue(second.succeeded, second)
        self.assertIn("isolated", second.stdout)

    def test_timeout_terminates_the_process(self) -> None:
        result = run_powershell("Start-Sleep -Seconds 5", timeout_seconds=1)
        self.assertTrue(result.timed_out, result)

    def test_working_directory_is_honored(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            result = run_powershell(
                "(Get-Location).Path | Write-Output",
                working_directory=directory,
                timeout_seconds=10,
            )
            self.assertTrue(result.succeeded, result)
            self.assertEqual(
                Path(result.stdout.strip()).resolve(),
                Path(directory).resolve(),
            )

    def test_connected_temp_values_override_saved_fallbacks(self) -> None:
        values = collect_temp_variables(
            3,
            '{"temp1": 11, "temp2": 22, "temp3": 33}',
            {"temp2": 220},
        )
        self.assertEqual(values, {"temp1": 11, "temp2": 220, "temp3": 33})

    def test_zero_temp_count_creates_no_assignments(self) -> None:
        self.assertEqual(collect_temp_variables(0, "{}", {"temp1": 9}), {})

    def test_temp_values_exist_only_inside_the_fresh_process(self) -> None:
        result = run_powershell(
            'Write-Output "$temp1,$temp2"',
            timeout_seconds=10,
            variables={"temp1": 4096, "temp2": -17},
        )
        self.assertTrue(result.succeeded, result)
        self.assertEqual(result.stdout.strip(), "4096,-17")

        fresh = run_powershell(
            "if (Test-Path variable:temp1) { exit 8 } else { Write-Output 'clean' }",
            timeout_seconds=10,
        )
        self.assertTrue(fresh.succeeded, fresh)
        self.assertEqual(fresh.stdout.strip(), "clean")

    def test_temp_wrapper_preserves_first_statement_requirements(self) -> None:
        result = run_powershell(
            "using namespace System.Text\nWrite-Output ([StringBuilder].FullName)\nWrite-Output $temp1",
            timeout_seconds=10,
            variables={"temp1": 27},
        )
        self.assertTrue(result.succeeded, result)
        normalized = result.stdout.replace("\r\n", "\n")
        self.assertIn("System.Text.StringBuilder\n27", normalized)

    def test_temp_values_reject_nonintegers(self) -> None:
        with self.assertRaisesRegex(ValueError, "temp1 must be an integer"):
            collect_temp_variables(1, '{"temp1": "4096"}', {})


if __name__ == "__main__":
    unittest.main()
