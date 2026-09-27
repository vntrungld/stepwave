from __future__ import annotations

from typer.testing import CliRunner

from stepwave_model.cli import app


def test_version() -> None:
    result = CliRunner().invoke(app, ["version"])
    assert result.exit_code == 0, result.output
    assert "stepwave-model" in result.output and "torch" in result.output


def test_help_lists_commands() -> None:
    result = CliRunner().invoke(app, ["--help"])
    assert result.exit_code == 0
    assert "version" in result.output
