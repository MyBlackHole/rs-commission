"""Real release WASM against API fixtures, not browser/backend/database E2E.

Also exercises the separately built tauri-transport WASM against an IPC fixture.
Python/fixture JavaScript are test tooling only, not product business code.
"""
import functools
import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import threading
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import sync_playwright, expect

ROOT = Path(os.environ.get("CONSOLE_WEB_ROOT", "apps/console/dist")).resolve()
OUT = Path(os.environ.get("CONSOLE_QA_OUTPUT", "target/browser-qa")).resolve()
OUT.mkdir(parents=True, exist_ok=True)
assert (ROOT / "index.html").is_file(), "Build the release Web bundle first"
CSP = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Content-Security-Policy", CSP)
        self.send_header("X-Content-Type-Options", "nosniff")
        super().end_headers()

    def log_message(self, *_args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(ROOT)))
threading.Thread(target=server.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{server.server_port}"
requests, writes, errors = [], [], []
write_attempts = {}
uid = "11111111-1111-4111-8111-111111111111"
actor = {"id": uid, "name": "测试管理员", "role": "admin", "account_id": None, "expires_at": "2099-01-01T00:00:00Z"}


def api(route):
    req = route.request
    path = req.url.split("/api/v1/")[-1].split("?")[0]
    offset = int(parse_qs(urlsplit(req.url).query).get("offset", ["0"])[0])
    requests.append({"method": req.method, "path": path, "offset": offset})
    status = 200
    if path == "me":
        data = actor
    elif path == "dashboard":
        data = {"order_count": 12, "paid_minor": "120000", "platform_net_minor": "7200", "available_minor": "111000", "frozen_minor": "9000", "reserved_minor": "0", "unknown_payouts": 0}
    elif path == "quotes":
        assert json.loads(req.post_data)["paid_minor"] == "10000"
        data = {"binding": False, "fee_pool_minor": "1000", "platform_minor": "600", "direct_minor": "300", "indirect_minor": "100", "merchant_minor": "9000"}
    elif path == "orders" and req.method == "GET":
        data = {"items": [{"id": uid, "external_id": "order-001", "merchant_id": uid, "currency": "CNY", "paid_minor": "10000", "commission_base_minor": "10000", "fee_pool_minor": "1000", "refunded_minor": "2500", "rule_id": uid, "rule_snapshot": {"version": 1}, "captured_at": "2026-09-25T00:00:00Z", "unlock_at": "2026-10-02T00:00:00Z", "released_at": None}], "limit": 50, "offset": offset, "has_more": False}
    elif path == f"orders/{uid}" and req.method == "GET":
        data = {
            "order": {"id": uid, "external_id": "order-001", "merchant_id": uid, "customer_external_id": "customer-001", "currency": "CNY", "paid_minor": "10000", "commission_base_minor": "10000", "fee_pool_minor": "1000", "refunded_minor": "2500", "rule_id": uid, "rule_snapshot": {"version": 1, "rate_bps": 1000}, "captured_at": "2026-09-25T00:00:00Z", "unlock_at": "2026-10-02T00:00:00Z", "released_at": None},
            "allocations": [{"order_id": uid, "ordinal": 0, "account_id": uid, "slot": "merchant", "original_minor": "9000", "refunded_minor": "2250"}, {"order_id": uid, "ordinal": 1, "account_id": uid, "slot": "platform", "original_minor": "1000", "refunded_minor": "250"}],
            "refunds": [{"id": uid, "external_id": "refund-001", "amount_minor": "2500", "cumulative_minor": "2500", "reason": "首次退款", "allocation_deltas": [], "created_at": "2026-09-25T01:00:00Z"}]
        }
    elif req.method == "POST":
        writes.append({"key": req.headers.get("idempotency-key"), "body": req.post_data, "url": req.url})
        write_attempts[path] = write_attempts.get(path, 0) + 1
        if write_attempts[path] == 1:
            status, data = 503, {"error": {"code": "retryable", "message": "测试：结果未知"}}
        elif path.endswith("/refunds"):
            data = {"id": uid, "external_id": "refund-002", "amount_minor": "1000", "cumulative_minor": "3500"}
        else:
            data = {"id": uid, "external_id": "merchant-001", "name": "<img src=x onerror=alert(1)>", "kind": "merchant", "parent_id": None, "active": True, "created_at": "2026-09-25T00:00:00Z"}
    elif path == "reconciliation":
        data = {"ok": True, "external_payment_reconciled": False}
    else:
        data = {"items": [{"id": uid, "external_id": f"page-{offset}", "name": "<img src=x onerror=alert(1)>", "available_minor": "9007199254740993", "status": "requested"}], "limit": 50, "offset": offset, "has_more": path == "accounts" and offset == 0}
    route.fulfill(status=status, content_type="application/json", body=json.dumps(data, ensure_ascii=False))


with sync_playwright() as p:
    executable = os.environ.get("CONSOLE_BROWSER") or shutil.which("google-chrome") or shutil.which("chromium")
    browser = p.chromium.launch(executable_path=executable, headless=True, args=["--no-sandbox"])
    context = browser.new_context(viewport={"width": 1440, "height": 1050}, device_scale_factor=1)
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.route("**/api/v1/**", api)
    page.goto(base, wait_until="networkidle")
    expect(page.get_by_role("button", name="安全登录")).to_be_visible(timeout=15000)
    page.screenshot(path=str(OUT / "login.png"), full_page=True)
    page.locator("input[type=password]").fill("ui-fixture-only-not-a-real-token")
    page.get_by_role("button", name="安全登录").click()
    expect(page.get_by_text("测试管理员", exact=True)).to_be_visible()
    expect(page.get_by_role("heading", name="业务总览", exact=True)).to_be_visible()
    expect(page.get_by_text("正在读取服务器数据…")).to_have_count(0)
    expect(page.get_by_role("button", name="下一页", exact=True)).to_be_disabled()
    assert "1_000_000" not in page.locator("body").inner_text()
    assert page.evaluate("localStorage.length===0 && sessionStorage.length===0")
    page.screenshot(path=str(OUT / "overview-desktop.png"), full_page=True)
    for title in ["账户管理", "推广关系", "抽佣规则", "订单管理", "佣金明细", "账户余额", "提现结算", "账本流水", "内部对账", "操作审计", "访问凭据", "可靠事件"]:
        page.locator("nav").get_by_role("button", name=title, exact=True).click()
        expect(page.get_by_role("heading", name=title, exact=True)).to_be_visible()
        expect(page.get_by_text("正在读取服务器数据…")).to_have_count(0)
    page.locator("nav").get_by_role("button", name="账户管理", exact=True).click()
    next_page = page.get_by_role("button", name="下一页", exact=True)
    expect(next_page).to_be_enabled()
    next_page.click()
    expect(page.locator("tbody")).to_contain_text("page-50")
    expect(next_page).to_be_disabled()
    page.get_by_role("button", name="上一页", exact=True).click()
    expect(page.locator("tbody")).to_contain_text("page-0")
    expect(page.get_by_role("button", name="上一页", exact=True)).to_be_disabled()
    assert sum(r["path"] == "accounts" and r["offset"] == 50 for r in requests) == 1
    assert page.locator("img").count() == 0

    # Dedicated order detail + refund workflow, separate from the generic JSON console.
    page.locator("nav").get_by_role("button", name="订单管理", exact=True).click()
    expect(page.get_by_role("button", name="查看详情", exact=True)).to_be_visible()
    page.get_by_role("button", name="查看详情", exact=True).click()
    expect(page.get_by_role("heading", name="订单详情", exact=True)).to_be_visible()
    expect(page.get_by_text("¥ 75.00", exact=True)).to_be_visible()
    expect(page.get_by_text("refund-001", exact=True)).to_be_visible()
    refund = page.locator(".refund-box")
    refund.locator("input").nth(0).fill("refund-002")
    refund.locator("input").nth(1).fill("10.00")
    refund.locator("textarea").fill("已核验渠道退款流水 RF002")
    refund.get_by_role("button", name="校验退款请求", exact=True).click()
    expect(page.locator("nav").get_by_role("button", name="业务总览", exact=True)).to_be_disabled()
    refund.locator("input[type=checkbox]").check()
    refund.get_by_role("button", name="确认退款", exact=True).click()
    expect(refund.get_by_role("button", name="以原幂等键重试", exact=True)).to_be_visible()
    expect(page.get_by_role("button", name="退出并清除会话")).to_be_disabled()
    refund.get_by_role("button", name="以原幂等键重试", exact=True).click()
    expect(refund.get_by_role("button", name="完成并刷新订单", exact=True)).to_be_visible()
    refund_pair = [w for w in writes if "/refunds" in w["url"]]
    assert len(refund_pair) == 2 and refund_pair[0]["key"] and refund_pair[0] == refund_pair[1]
    body = json.loads(refund_pair[0]["body"])
    assert body == {"external_id": "refund-002", "amount_minor": "1000", "reason": "已核验渠道退款流水 RF002"}
    refund.get_by_role("button", name="完成并刷新订单", exact=True).click()
    expect(page.locator("nav").get_by_role("button", name="业务总览", exact=True)).to_be_enabled()

    page.locator("nav").get_by_role("button", name="佣金试算", exact=True).click()
    panel = page.locator("section.panel").first
    panel.locator("input").nth(0).fill(uid)
    page.get_by_role("button", name="向服务器试算").click()
    expect(panel.locator("pre")).to_contain_text("binding")
    op = page.locator("section.operations")
    op.get_by_role("button", name="校验并准备请求").click()
    expect(op.locator("textarea")).to_be_disabled()
    expect(op.get_by_role("button", name="确认提交", exact=True)).to_be_disabled()
    op.locator("input[type=checkbox]").check()
    op.get_by_role("button", name="确认提交", exact=True).click()
    expect(op.get_by_role("button", name="以原幂等键重试")).to_be_visible()
    expect(op.get_by_role("button", name="返回编辑 / 新操作")).to_be_disabled()
    expect(page.get_by_role("button", name="退出并清除会话")).to_be_disabled()
    op.get_by_role("button", name="以原幂等键重试").click()
    expect(op.get_by_role("button", name="返回编辑 / 新操作")).to_be_enabled()
    op.get_by_role("button", name="返回编辑 / 新操作").click()
    expect(op.locator("textarea")).to_be_enabled()
    expect(page.locator("nav").get_by_role("button", name="业务总览", exact=True)).to_be_enabled()
    generic_pair = [w for w in writes if w["url"].endswith("/api/v1/accounts")]
    assert len(generic_pair) == 2 and generic_pair[0]["key"] and generic_pair[0] == generic_pair[1]
    assert page.locator("img").count() == 0
    page.set_viewport_size({"width": 390, "height": 844})
    page.locator("nav").get_by_role("button", name="业务总览", exact=True).click()
    expect(page.get_by_role("heading", name="业务总览", exact=True)).to_be_visible()
    expect(page.get_by_text("正在读取服务器数据…")).to_have_count(0)
    page.screenshot(path=str(OUT / "overview-mobile.png"), full_page=True)
    assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth")
    page.get_by_role("button", name="退出并清除会话").click()
    expect(page.get_by_role("button", name="安全登录")).to_be_visible()
    assert page.locator("input[type=password]").input_value() == ""
    assert not errors, errors
    result = {"tested_ref": os.environ.get("GITHUB_SHA", "local"), "mode": "Chromium, release WASM, mocked API (not backend E2E)", "checks": ["CSP load", "login/logout", "no browser token persistence", "13 data views", "dedicated order detail", "dedicated refund confirmation and same-key retry", "pagination advances and reverses offset", "no template expression leakage", "server quote payload", "escaped text", "prepare locks payload", "explicit confirmation", "503 preserves request and prevents logout", "same-key same-body retry", "390px responsive width"], "requests": len(requests), "writes": len(writes), "page_errors": errors}
    (OUT / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    browser.close()
server.shutdown()
subprocess.run([sys.executable, str(Path(__file__).with_name("tauri_transport_browser.py")), str(ROOT.parent / "dist-tauri"), str(OUT)], check=True)
