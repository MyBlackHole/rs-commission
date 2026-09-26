"""Real browser -> Nginx -> Axum -> PostgreSQL E2E.

No API routes are mocked in this test. The database is ephemeral CI data and all
financial operations use stable idempotency keys. Secrets are never written to
the result artifact.
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import secrets
import shutil
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from playwright.sync_api import expect, sync_playwright


BASE = os.environ.get("E2E_BASE_URL", "http://127.0.0.1:8090").rstrip("/")
ADMIN = os.environ["ADMIN_TOKEN"]
OUT = Path(os.environ.get("REAL_E2E_OUTPUT", "target/real-e2e")).resolve()
OUT.mkdir(parents=True, exist_ok=True)


def api(method: str, path: str, token: str, body=None, key: str | None = None):
    """Use the same Nginx origin as the browser; retry 5xx only with the same key/body."""
    raw = None if body is None else json.dumps(body, ensure_ascii=False).encode()
    headers = {"Authorization": f"Bearer {token}"}
    if raw is not None:
        headers["Content-Type"] = "application/json"
    if key is not None:
        headers["Idempotency-Key"] = key
    request = Request(f"{BASE}/api/v1{path}", data=raw, headers=headers, method=method)
    for attempt in range(3):
        try:
            with urlopen(request, timeout=15) as response:
                return json.loads(response.read())
        except HTTPError as error:
            payload = error.read()
            try:
                detail = json.loads(payload)
            except Exception:
                detail = {"error": {"code": "http_error", "message": f"HTTP {error.code}"}}
            if error.code >= 500 and key is not None and attempt < 2:
                time.sleep(0.2)
                continue
            code = detail.get("error", {}).get("code", "http_error")
            message = detail.get("error", {}).get("message", f"HTTP {error.code}")
            raise AssertionError(f"{method} {path}: HTTP {error.code} / {code}: {message}") from error
    raise AssertionError(f"{method} {path}: retry budget exhausted")


def post(path: str, token: str, body, key: str):
    return api("POST", path, token, body, key)


# Seed the minimum real business state through the public HTTP API.
finance_secret = "cms_" + secrets.token_hex(32)
post(
    "/credentials",
    ADMIN,
    {
        "name": "E2E 财务",
        "role": "finance",
        "account_id": None,
        "expires_in_days": 1,
        "secret": finance_secret,
    },
    "e2e-create-finance-v1",
)
merchant = post(
    "/accounts",
    ADMIN,
    {
        "external_id": "e2e-merchant",
        "name": "E2E 商家",
        "kind": "merchant",
        "parent_id": None,
    },
    "e2e-create-merchant-v1",
)
merchant_id = merchant["id"]
post(
    "/rules",
    ADMIN,
    {
        "name": "E2E 默认规则",
        "merchant_id": None,
        "priority": 0,
        "min_base_minor": "0",
        "max_base_minor": None,
        "terms": {
            "rate_bps": 1000,
            "fixed_minor": "0",
            "cap_minor": None,
            "direct_bps": 0,
            "indirect_bps": 0,
            "freeze_seconds": 0,
        },
        "effective_from": None,
        "effective_until": None,
    },
    "e2e-create-rule-v1",
)
captured = post(
    "/orders",
    ADMIN,
    {
        "external_id": "e2e-order-001",
        "currency": "CNY",
        "merchant_id": merchant_id,
        "customer_external_id": None,
        "paid_minor": "10000",
        "commission_base_minor": "10000",
    },
    "e2e-capture-order-v1",
)
order_id = captured["order"]["id"]
post(f"/orders/{order_id}/release", ADMIN, {}, "e2e-release-order-v1")
payout = post(
    "/payouts",
    ADMIN,
    {
        "external_id": "e2e-payout-001",
        "account_id": merchant_id,
        "amount_minor": "500",
        "destination_ref": "verified-e2e-payee",
    },
    "e2e-request-payout-v1",
)
payout_id = payout["id"]


with sync_playwright() as playwright:
    executable = (
        os.environ.get("CONSOLE_BROWSER")
        or shutil.which("google-chrome")
        or shutil.which("chromium")
    )
    browser = playwright.chromium.launch(
        executable_path=executable, headless=True, args=["--no-sandbox"]
    )
    page = browser.new_page(viewport={"width": 1440, "height": 1050})
    page_errors: list[str] = []
    page.on("pageerror", lambda error: page_errors.append(str(error)))

    # Admin: use the real order-detail page and post a real refund fact.
    page.goto(BASE, wait_until="networkidle")
    expect(page.get_by_role("button", name="安全登录")).to_be_visible(timeout=15000)
    page.locator("input[type=password]").fill(ADMIN)
    page.get_by_role("button", name="安全登录").click()
    expect(page.get_by_text("初始管理员", exact=True)).to_be_visible()

    page.locator("nav").get_by_role("button", name="订单管理", exact=True).click()
    expect(page.get_by_role("heading", name="订单管理", exact=True)).to_be_visible()
    page.get_by_role("button", name="查看详情", exact=True).first.click()
    expect(page.get_by_role("heading", name="订单详情", exact=True)).to_be_visible()
    expect(page.get_by_text("e2e-order-001", exact=True)).to_be_visible()

    refund = page.locator(".refund-box")
    refund.locator("input").nth(0).fill("e2e-refund-001")
    refund.locator("input").nth(1).fill("10.00")
    refund.locator("textarea").fill("E2E 已核验退款事实")
    refund.get_by_role("button", name="校验退款请求", exact=True).click()
    refund.locator("input[type=checkbox]").check()
    refund.get_by_role("button", name="确认退款", exact=True).click()
    expect(refund.get_by_role("button", name="完成并刷新订单", exact=True)).to_be_visible()
    refund.get_by_role("button", name="完成并刷新订单", exact=True).click()
    expect(page.get_by_text("e2e-refund-001", exact=True)).to_be_visible()
    page.screenshot(path=str(OUT / "real-order-refund.png"), full_page=True)

    page.get_by_role("button", name="退出并清除会话").click()
    expect(page.get_by_role("button", name="安全登录")).to_be_visible()

    # Finance: read the same real payout then advance its real server state.
    page.locator("input[type=password]").fill(finance_secret)
    page.get_by_role("button", name="安全登录").click()
    expect(page.get_by_text("E2E 财务", exact=True)).to_be_visible()

    page.locator("nav").get_by_role("button", name="提现结算", exact=True).click()
    expect(page.get_by_role("heading", name="提现结算", exact=True)).to_be_visible()
    page.get_by_role("button", name="查看详情", exact=True).first.click()
    expect(page.get_by_role("heading", name="提现详情", exact=True)).to_be_visible()
    expect(
        page.locator(".payout-detail article.metric")
        .filter(has_text="状态")
        .locator("strong")
    ).to_have_text("待审核")

    approve = page.locator(".payout-action").filter(has_text="通过审核")
    approve.get_by_role("button", name="准备通过审核", exact=True).click()
    approve.locator("input[type=checkbox]").check()
    approve.get_by_role("button", name="确认提交", exact=True).click()
    expect(
        approve.get_by_role("button", name="完成并刷新提现", exact=True)
    ).to_be_visible()
    approve.get_by_role("button", name="完成并刷新提现", exact=True).click()
    expect(
        page.locator(".payout-detail article.metric")
        .filter(has_text="状态")
        .locator("strong")
    ).to_have_text("已审核")
    page.screenshot(path=str(OUT / "real-payout-approved.png"), full_page=True)

    assert not page_errors, page_errors
    browser.close()


# Verify durable server/database state through the same gateway after UI actions.
order = api("GET", f"/orders/{order_id}", ADMIN)
assert order["order"]["refunded_minor"] == "1000", order
assert [r["external_id"] for r in order["refunds"]] == ["e2e-refund-001"], order
payout_after = api("GET", f"/payouts/{payout_id}", finance_secret)
assert payout_after["status"] == "approved", payout_after

result = {
    "tested_ref": os.environ.get("GITHUB_SHA", "local"),
    "mode": "real Chromium -> Nginx -> Axum -> PostgreSQL; no API mocks",
    "checks": [
        "real admin authentication",
        "real order list/detail",
        "real refund posting and ledger transaction",
        "refund state persisted in PostgreSQL",
        "real finance authentication",
        "real payout list/detail",
        "real payout approval state transition",
        "member-independent server authorization still exercised by Rust integration suite",
    ],
    "order_id": order_id,
    "payout_id": payout_id,
    "refund_external_id": "e2e-refund-001",
    "payout_status": payout_after["status"],
    "page_errors": page_errors,
}
(OUT / "result.json").write_text(
    json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
)
print(json.dumps(result, ensure_ascii=False, indent=2))
