"""Test helper (run as a script): open a marimo notebook in a real edit-mode kernel, wait for
the first run, then write a command file and wait for a cell output containing `expect`.

This drives marimo's own session machinery the way `marimo export html` does
(marimo/_export/file.py `run_notebook`, 0.25.2); those internals are private, so any
ImportError/TypeError from them exits 77 and the test is skipped rather than failed.

usage: marimo_session.py NOTEBOOK COMMAND_FILE COMMAND_TEXT EXPECT [COMMAND_TEXT EXPECT ...]

With COMMAND_FILE `--exec`, each COMMAND_TEXT is a JSON argv list that is run (in the
notebook's folder) instead of written, e.g. `arb inspect done t001 --file answer.json`.
"""
import asyncio, json, sys
from pathlib import Path


async def main(nb: str, cmdfile: str, steps: list[tuple[str, str]], timeout: float = 40) -> int:
    from marimo._config.manager import get_default_config_manager
    from marimo._messaging.notification import CellNotification, CompletedRunNotification
    from marimo._messaging.serde import deserialize_kernel_message
    from marimo._runtime.commands import AppMetadata
    from marimo._session.consumer import SessionConsumer
    from marimo._session.model import ConnectionState, SessionMode
    from marimo._session.notebook import load_notebook
    from marimo._session.requests import InstantiateNotebookRequest
    from marimo._session.session import SessionImpl
    from marimo._session.startup import SessionStartup
    from marimo._types.ids import ConsumerId

    done = asyncio.Event()
    seen = asyncio.Event()
    outputs: list[str] = []
    current = [""]
    loop = asyncio.get_running_loop()

    class Consumer(SessionConsumer):
        @property
        def consumer_id(self):
            return ConsumerId("default")

        def notify(self, notification):
            data = deserialize_kernel_message(notification)
            if isinstance(data, CellNotification) and data.output is not None:
                text = json.dumps(data.output.data, ensure_ascii=False) \
                    if not isinstance(data.output.data, str) else data.output.data
                outputs.append(text)
                if current[0] and current[0] in text:
                    loop.call_soon_threadsafe(seen.set)
            if isinstance(data, CompletedRunNotification):
                loop.call_soon_threadsafe(done.set)

        def on_attach(self, session, event_bus):
            pass

        def on_detach(self):
            return None

        def connection_state(self):
            return ConnectionState.OPEN

    fm = load_notebook(str(Path(nb).absolute()))
    config = get_default_config_manager(current_path=fm.path).with_overrides(
        {"runtime": {"on_cell_change": "autorun", "auto_instantiate": True,
                     "auto_reload": "off", "watcher_on_save": "lazy"}})
    session = await SessionImpl.create(
        startup=SessionStartup(), initialization_id="_any_", session_consumer=Consumer(),
        mode=SessionMode.EDIT,
        app_metadata=AppMetadata(query_params={}, filename=fm.path, cli_args={}, argv=None,
                                 app_config=fm.app.config),
        app_file_manager=fm, config_manager=config, virtual_file_storage=None,
        redirect_console_to_browser=False, ttl_seconds=None, auto_instantiate=True)
    try:
        session.instantiate(InstantiateNotebookRequest(object_ids=[], values=[]), http_request=None)
        await asyncio.wait_for(done.wait(), timeout)
        for cmdtext, expect in steps:
            await asyncio.sleep(1.5)  # let the cell's watcher take its first look
            seen.clear()
            current[0] = expect
            if cmdfile == "--exec":  # without blocking the loop the session reports on
                proc = await asyncio.create_subprocess_exec(
                    *json.loads(cmdtext), cwd=Path(nb).parent, stdin=asyncio.subprocess.DEVNULL,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                out, err = await asyncio.wait_for(proc.communicate(), timeout)
                if proc.returncode != 0:
                    print("EXEC-FAILED", out.decode(), err.decode())
                    return 1
            else:
                Path(cmdfile).parent.mkdir(parents=True, exist_ok=True)
                Path(cmdfile).write_text(cmdtext)
            await asyncio.wait_for(seen.wait(), timeout)
            print("RERUN-OK", expect)
        return 0
    except asyncio.TimeoutError:
        print("TIMEOUT", outputs[-5:])
        return 1
    finally:
        session.close()


if __name__ == "__main__":
    args = sys.argv[1:]
    pairs = list(zip(args[2::2], args[3::2]))
    try:
        code = asyncio.run(main(args[0], args[1], pairs))
    except (ImportError, TypeError, AttributeError) as exc:
        print(f"SKIP marimo internals changed: {exc!r}")
        code = 77
    sys.exit(code)
