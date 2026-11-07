"""The Mac app's agent drawer (ACP): source scans that keep its guards in place.

The drawer is Swift + a web page and is checked live with `ARB_PROBE=drawer` (app/README.md);
these tests only make sure nobody quietly removes what it must never do.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

APP = Path(__file__).parents[1] / "app"
ACP = (APP / "Sources/Arboria/Acp.swift").read_text()
SHELL = (APP / "Sources/Arboria/Shell.swift").read_text()
JS = (APP / "web/drawer.js").read_text()


def test_never_bypass_permissions() -> None:
    assert 'static let forbiddenMode = "bypassPermissions"' in ACP
    # the session is created with bypass disallowed, and the mode list and set_mode refuse it
    assert '"allowDangerouslySkipPermissions": false' in ACP
    assert '($0["id"] as? String) != Acp.forbiddenMode' in ACP
    assert "guard let sid = sessionId, id != Acp.forbiddenMode" in ACP
    assert 'u["currentModeId"] as? String == Acp.forbiddenMode' in ACP
    assert '!text.contains("bypass")' in ACP  # permission options that would switch to it


def test_permission_answers_only_for_pending_requests() -> None:
    assert "guard let allowed = permissions.removeValue(forKey: id) else { return }" in ACP
    assert "allowed.contains(o)" in ACP


def test_no_file_or_terminal_access_through_the_app() -> None:
    assert '"fs": ["readTextFile": false, "writeTextFile": false], "terminal": false' in ACP
    assert '"not supported by Arboria"' in ACP  # any other request from the agent is refused


def test_users_own_claude_and_a_clean_environment() -> None:
    assert "CLAUDE_CODE_EXECUTABLE" in ACP and "proc.environment = cleanEnvironment()" in ACP
    arbd = (APP / "Sources/Arboria/Arbd.swift").read_text()
    assert '!$0.key.hasPrefix("CLAUDE") || $0.key == "CLAUDE_CONFIG_DIR"' in arbd


def test_adapter_pinned_without_its_bundled_claude() -> None:
    pkg = json.loads((APP / "acp/package.json").read_text())
    version = pkg["dependencies"]["@agentclientprotocol/claude-agent-acp"]
    assert re.fullmatch(r"\d+\.\d+\.\d+", version), "pin an exact version"
    lock = json.loads((APP / "acp/package-lock.json").read_text())
    assert lock["packages"]["node_modules/@agentclientprotocol/claude-agent-acp"]["version"] == version
    build = (APP / "build.sh").read_text()
    assert "npm ci" in build and "--omit=optional" in build


def test_drawer_page_draws_agent_text_as_text() -> None:
    assert "innerHTML" not in JS and "insertAdjacentHTML" not in JS
    # the page relays; the drawer's message handler has no approval or arbd call
    drawer = SHELL[SHELL.index("final class Drawer"):SHELL.index("final class Shell")]
    assert "arbd" not in drawer and "approve" not in drawer
