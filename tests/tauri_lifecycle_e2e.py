"""Real Tauri desktop lifecycle E2E on Linux and Windows.

The same W3C test drives the compiled Tauri binary through tauri-driver and the
platform-native WebDriver (WebKitWebDriver on Linux, EdgeDriver on Windows).
It hard-kills the real application after a 503/unknown write, restarts a fresh
Tauri process with the same app-data directory, re-authenticates, and verifies
that the original idempotency key and request body are recovered.
"""
from __future__ import annotations

import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


APP = Path(sys.argv[1]).resolve()
OUT = Path(os.environ.get("TAURI_LIFECYCLE_OUTPUT", "target/tauri-lifecycle-e2e")).resolve()
OUT.mkdir(parents=True, exist_ok=True)
TOKEN = "tauri-lifecycle-fixture-secret"
ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf"
WINDOWS = os.name == "nt"
NATIVE_DRIVER = os.environ.get("TAURI_NATIVE_DRIVER")


class FixtureState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.writes: list[dict[str, str]] = []

    def record(self, item: dict[str, str]) -> int:
        with self.lock:
            self.writes.append(item)
            return len(self.writes)


STATE = FixtureState()


class ApiHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args) -> None:
        pass

    def send_json(self, status: int, value: object) -> None:
        data = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        if path == "/api/v1/me":
            self.send_json(
                200,
                {
                    "id": "11111111-1111-4111-8111-111111111111",
                    "name": "Tauri生命周期管理员",
                    "role": "admin",
                    "account_id": None,
                    "expires_at": "2099-01-01T00:00:00Z",
                },
            )
            return
        if path == "/api/v1/dashboard":
            self.send_json(
                200,
                {
                    "order_count": 0,
                    "paid_minor": "0",
                    "platform_net_minor": "0",
                    "available_minor": "0",
                    "frozen_minor": "0",
                    "reserved_minor": "0",
                    "unknown_payouts": 0,
                },
            )
            return
        self.send_json(404, {"error": {"code": "not_found", "message": "fixture path"}})

    def do_POST(self) -> None:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length).decode("utf-8")
        item = {
            "path": self.path.split("?", 1)[0],
            "key": self.headers.get("Idempotency-Key", ""),
            "body": body,
        }
        attempt = STATE.record(item)
        if item["path"] != "/api/v1/accounts":
            self.send_json(404, {"error": {"code": "not_found", "message": "fixture path"}})
            return
        if attempt == 1:
            self.send_json(
                503,
                {"error": {"code": "fixture_unknown", "message": "模拟结果未知"}},
            )
            return
        self.send_json(
            200,
            {
                "id": "22222222-2222-4222-8222-222222222222",
                "external_id": "merchant-001",
                "name": "示例商家",
                "kind": "merchant",
                "parent_id": None,
                "active": True,
                "created_at": "2026-09-27T00:00:00Z",
            },
        )


class DriverError(RuntimeError):
    pass


class W3C:
    def __init__(
        self,
        base: str = "http://127.0.0.1:4444",
        webview_user_data: Path | None = None,
    ) -> None:
        self.base = base.rstrip("/")
        self.session: str | None = None
        self.webview_user_data = webview_user_data

    def request(self, method: str, path: str, payload: object | None = None, timeout: int = 30):
        data = None
        headers = {}
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(self.base + path, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as response:
                raw = response.read()
        except urllib.error.HTTPError as error:
            raw = error.read()
            try:
                detail = json.loads(raw)
            except Exception:
                detail = raw.decode("utf-8", "replace")
            raise DriverError(f"{method} {path}: HTTP {error.code}: {detail}") from error
        except OSError as error:
            raise DriverError(f"{method} {path}: {error}") from error
        if not raw:
            return {}
        result = json.loads(raw)
        value = result.get("value")
        if isinstance(value, dict) and value.get("error"):
            raise DriverError(f"{method} {path}: {value}")
        return result

    def start(self) -> None:
        tauri_options: dict[str, object] = {"application": str(APP)}
        if WINDOWS and self.webview_user_data is not None:
            self.webview_user_data.mkdir(parents=True, exist_ok=True)
            tauri_options["webviewOptions"] = {
                "userDataFolder": str(self.webview_user_data)
            }
        result = self.request(
            "POST",
            "/session",
            {
                "capabilities": {
                    "alwaysMatch": {
                        "browserName": "wry",
                        "tauri:options": tauri_options,
                    }
                }
            },
            timeout=120 if WINDOWS else 60,
        )
        value = result.get("value", {})
        self.session = value.get("sessionId") or result.get("sessionId")
        if not self.session:
            raise DriverError(f"webdriver did not return session id: {result}")

    def close(self) -> None:
        if not self.session:
            return
        try:
            self.request("DELETE", f"/session/{self.session}", timeout=10)
        except DriverError:
            pass
        self.session = None

    def find(self, using: str, value: str, timeout: float = 15.0) -> str:
        assert self.session
        deadline = time.monotonic() + timeout
        last: Exception | None = None
        while time.monotonic() < deadline:
            try:
                result = self.request(
                    "POST",
                    f"/session/{self.session}/element",
                    {"using": using, "value": value},
                    timeout=5,
                )
                element = result.get("value", {})
                found = element.get(ELEMENT_KEY)
                if found:
                    return found
            except DriverError as error:
                last = error
            time.sleep(0.2)
        raise DriverError(f"element not found: {using}={value}; last={last}")

    def find_xpath(self, value: str, timeout: float = 15.0) -> str:
        return self.find("xpath", value, timeout)

    def find_css(self, value: str, timeout: float = 15.0) -> str:
        return self.find("css selector", value, timeout)

    def click(self, element: str) -> None:
        assert self.session
        self.request("POST", f"/session/{self.session}/element/{element}/click", {})

    def clear(self, element: str) -> None:
        assert self.session
        self.request("POST", f"/session/{self.session}/element/{element}/clear", {})

    def type(self, element: str, text: str) -> None:
        assert self.session
        self.request(
            "POST",
            f"/session/{self.session}/element/{element}/value",
            {"text": text},
        )

    def fill(self, selector: str, text: str) -> None:
        element = self.find_css(selector)
        self.clear(element)
        self.type(element, text)

    def text(self, element: str) -> str:
        assert self.session
        result = self.request("GET", f"/session/{self.session}/element/{element}/text")
        return str(result.get("value", ""))

    def wait_absent_xpath(self, value: str, timeout: float = 15.0) -> None:
        assert self.session
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                result = self.request(
                    "POST",
                    f"/session/{self.session}/element",
                    {"using": "xpath", "value": value},
                    timeout=3,
                )
                if result.get("value", {}).get(ELEMENT_KEY):
                    time.sleep(0.2)
                    continue
            except DriverError:
                return
        raise DriverError(f"element still present: {value}")


def wait_webdriver_ready(port: int, timeout: float = 20.0) -> None:
    deadline = time.monotonic() + timeout
    url = f"http://127.0.0.1:{port}/status"
    last: Exception | None = None
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as response:
                if 200 <= response.status < 300:
                    response.read()
                    return
        except (OSError, urllib.error.URLError, urllib.error.HTTPError) as error:
            last = error
        time.sleep(0.2)
    raise RuntimeError(f"WebDriver on port {port} did not become ready: {last}")


def app_pids() -> list[int]:
    if WINDOWS:
        quoted = str(APP).replace("'", "''")
        command = (
            "$target=[System.IO.Path]::GetFullPath('"
            + quoted
            + "'); "
            "Get-CimInstance Win32_Process | "
            "Where-Object { $_.ExecutablePath -and "
            "[System.IO.Path]::GetFullPath($_.ExecutablePath) -ieq $target } | "
            "ForEach-Object { $_.ProcessId }"
        )
        completed = subprocess.run(
            ["powershell", "-NoProfile", "-Command", command],
            text=True,
            capture_output=True,
            check=True,
        )
        return [
            int(line.strip())
            for line in completed.stdout.splitlines()
            if line.strip().isdigit()
        ]

    result = []
    me = os.getpid()
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        if pid == me:
            continue
        try:
            exe = Path(os.readlink(entry / "exe")).resolve()
        except (FileNotFoundError, PermissionError, OSError):
            continue
        if exe == APP:
            result.append(pid)
    return result


def kill_real_app() -> list[int]:
    pids = app_pids()
    if not pids:
        raise RuntimeError("real Tauri application process was not found")
    for pid in pids:
        if WINDOWS:
            subprocess.run(
                ["taskkill", "/PID", str(pid), "/F", "/T"],
                text=True,
                capture_output=True,
                check=False,
            )
        else:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and app_pids():
        time.sleep(0.1)
    if app_pids():
        raise RuntimeError("Tauri application survived hard kill")
    return pids


def wait_port_closed(port: int, timeout: float = 10.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with socket.socket() as sock:
            sock.settimeout(0.2)
            if sock.connect_ex(("127.0.0.1", port)) != 0:
                return
        time.sleep(0.1)
    raise RuntimeError(f"port {port} did not close")


def start_tauri_driver(data_home: Path, log_name: str):
    env = os.environ.copy()
    if not WINDOWS:
        env["XDG_DATA_HOME"] = str(data_home)
        env["XDG_CACHE_HOME"] = str(data_home.parent / "cache")
        env.setdefault("GDK_BACKEND", "x11")
        env.setdefault("WEBKIT_DISABLE_COMPOSITING_MODE", "1")

    command = ["tauri-driver", "--port", "4444", "--native-port", "4445"]
    if NATIVE_DRIVER:
        command += ["--native-driver", NATIVE_DRIVER]

    popen_args = {
        "stdout": None,
        "stderr": subprocess.STDOUT,
        "env": env,
    }
    if WINDOWS:
        popen_args["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_args["start_new_session"] = True

    log = open(OUT / log_name, "wb")
    popen_args["stdout"] = log
    process = subprocess.Popen(command, **popen_args)
    try:
        # /status is a real WebDriver request; a raw TCP probe leaves an
        # incomplete HTTP connection in tauri-driver and is especially harmful
        # on the Windows/EdgeDriver startup path.
        wait_webdriver_ready(4444)
    except Exception:
        process.kill()
        process.wait(timeout=5)
        log.close()
        raise
    return process, log


def kill_driver(process: subprocess.Popen, log) -> None:
    if process.poll() is None:
        if WINDOWS:
            subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/F", "/T"],
                text=True,
                capture_output=True,
                check=False,
            )
        else:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    log.close()
    wait_port_closed(4444)
    wait_port_closed(4445)


def login(client: W3C, origin: str) -> None:
    client.find_xpath("//button[normalize-space(.)='安全登录']")
    client.fill("#api-origin", origin)
    client.fill("input[type='password']", TOKEN)
    client.click(client.find_xpath("//button[normalize-space(.)='安全登录']"))
    client.find_xpath("//*[normalize-space(.)='Tauri生命周期管理员']", timeout=20)


def recovery_files(data_home: Path) -> list[Path]:
    return list(data_home.rglob("pending-write-v1.json"))


def clear_recovery_files(data_home: Path) -> None:
    for name in ("pending-write-v1.json", "pending-write-v1.next"):
        for path in data_home.rglob(name):
            try:
                path.unlink()
            except FileNotFoundError:
                pass


def wait_recovery_files(data_home: Path, expected: int, timeout: float = 10.0) -> list[Path]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        files = recovery_files(data_home)
        if len(files) == expected:
            return files
        time.sleep(0.1)
    raise RuntimeError(f"expected {expected} recovery files, got {recovery_files(data_home)}")


def main() -> None:
    if not APP.is_file():
        raise SystemExit(f"Tauri executable not found: {APP}")

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ApiHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"

    with tempfile.TemporaryDirectory(prefix="rs-commission-tauri-") as temp:
        if WINDOWS:
            data_home = Path(os.environ["APPDATA"]) / "com.rscommission.console"
        else:
            data_home = Path(temp) / "data"
            data_home.mkdir(parents=True)
        clear_recovery_files(data_home)
        first_process = second_process = None
        first_log = second_log = None
        first_client = second_client = None
        killed_pids: list[int] = []
        try:
            first_process, first_log = start_tauri_driver(data_home, "tauri-driver-first.log")
            first_client = W3C(
                webview_user_data=Path(temp) / "webview-first"
            )
            first_client.start()
            login(first_client, origin)

            first_client.click(
                first_client.find_xpath("//button[normalize-space(.)='校验并准备请求']")
            )
            first_client.click(first_client.find_css("section.operations input[type='checkbox']"))
            first_client.click(
                first_client.find_xpath("//button[normalize-space(.)='确认提交']")
            )
            first_client.find_xpath(
                "//button[normalize-space(.)='以原幂等键重试']", timeout=20
            )
            request_info = first_client.text(first_client.find_css("section.operations pre.request-info"))
            if "Idempotency-Key:" not in request_info:
                raise RuntimeError(f"missing idempotency key in UI: {request_info}")
            displayed_key = request_info.split("Idempotency-Key:", 1)[1].strip()
            files = wait_recovery_files(data_home, 1)
            persisted = files[0].read_text(encoding="utf-8")
            if TOKEN in persisted:
                raise RuntimeError("bearer token leaked into durable recovery file")
            if displayed_key not in persisted:
                raise RuntimeError("durable recovery file does not contain displayed key")

            killed_pids = kill_real_app()
            kill_driver(first_process, first_log)
            first_process = first_log = None
            first_client = None

            second_process, second_log = start_tauri_driver(data_home, "tauri-driver-second.log")
            second_client = W3C(
                webview_user_data=Path(temp) / "webview-second"
            )
            second_client.start()
            login(second_client, origin)

            second_client.find_xpath(
                "//*[normalize-space(.)='恢复未完成写请求']", timeout=20
            )
            recovered_info = second_client.text(
                second_client.find_css(".recovery-panel pre.request-info")
            )
            if displayed_key not in recovered_info:
                raise RuntimeError(
                    f"restarted Tauri did not recover original key: {recovered_info}"
                )
            second_client.click(
                second_client.find_css(".recovery-panel input[type='checkbox']")
            )
            second_client.click(
                second_client.find_xpath("//button[normalize-space(.)='以原幂等键重试']")
            )
            second_client.find_xpath(
                "//button[normalize-space(.)='清除恢复记录']", timeout=20
            )
            second_client.click(
                second_client.find_xpath("//button[normalize-space(.)='清除恢复记录']")
            )
            second_client.wait_absent_xpath(
                "//*[normalize-space(.)='恢复未完成写请求']", timeout=15
            )
            wait_recovery_files(data_home, 0)

            with STATE.lock:
                writes = list(STATE.writes)
            if len(writes) != 2:
                raise RuntimeError(f"expected exactly two writes, got {writes}")
            if writes[0] != writes[1]:
                raise RuntimeError("restart retry changed path, key, or body")
            if writes[0]["key"] != displayed_key:
                raise RuntimeError("server idempotency key differs from UI recovery key")

            result = {
                "tested_ref": os.environ.get("GITHUB_SHA", "local"),
                "mode": (
                    "real Tauri desktop + tauri-driver/EdgeDriver + taskkill/restart"
                    if WINDOWS
                    else "real Tauri desktop + tauri-driver/WebKitWebDriver + SIGKILL/restart"
                ),
                "platform": "windows" if WINDOWS else "linux",
                "app_processes_killed": len(killed_pids),
                "first_attempt_unknown": True,
                "restarted_process_recovered_original_key": True,
                "same_path_key_body_after_restart": True,
                "bearer_absent_from_recovery_file": True,
                "recovery_file_removed_after_resolution": True,
                "writes": len(writes),
                "passed": True,
            }
            (OUT / "result.json").write_text(
                json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
            )
            print(json.dumps(result, ensure_ascii=False, indent=2))
        finally:
            if second_client is not None:
                second_client.close()
            if first_client is not None:
                first_client.close()
            if second_process is not None and second_log is not None:
                kill_driver(second_process, second_log)
            if first_process is not None and first_log is not None:
                kill_driver(first_process, first_log)
            clear_recovery_files(data_home)
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
