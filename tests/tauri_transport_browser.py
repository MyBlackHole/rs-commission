"""Real Leptos tauri-feature WASM with an IPC fixture, NOT a Tauri runtime test."""
from pathlib import Path
import functools
import http.server
import threading
import json
import os
import shutil
import sys
from playwright.sync_api import sync_playwright, expect

ROOT = Path(sys.argv[1]).resolve()
OUT = Path(sys.argv[2]).resolve()
OUT.mkdir(parents=True, exist_ok=True)
assert (ROOT / "index.html").is_file(), "Build dist-tauri before this test"


class Handler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def end_headers(self):
        self.send_header("Content-Security-Policy", "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'")
        super().end_headers()


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(ROOT)))
threading.Thread(target=server.serve_forever, daemon=True).start()
stub = r'''
(() => {
 const id='11111111-1111-4111-8111-111111111111';
 const write='22222222-2222-4222-8222-222222222222';
 let attempts=0;
 window.fixtureCalls=[];
 window.__TAURI__={core:{invoke:async(command,args)=>{
   window.fixtureCalls.push({command,args});
   if (!args || args.constructor!==Object) throw 'IPC arguments must be JSON objects';
   if(command==='session_login') return {id,actor:{id,name:'IPC测试管理员',role:'admin',account_id:null,expires_at:'2099-01-01T00:00:00Z'}};
   if(args.session!==id) throw 'wrong session';
   if(command==='read_resource') { if(args.resource==='orders') return {items:[{id,external_id:'order-001',paid_minor:'10000',refunded_minor:'0',captured_at:'2026-09-25T00:00:00Z',released_at:null}],has_more:false,offset:0,limit:50}; return {items:[],has_more:false,offset:0,limit:50}; }
   if(command==='quote_commission') return {binding:false,fee_pool_minor:'1000'};
   if(command==='order_detail') { if(args.order!==id) throw 'wrong order handle'; return {order:{id,external_id:'order-001',paid_minor:'10000',refunded_minor:'0',fee_pool_minor:'1000',unlock_at:'2026-10-02T00:00:00Z',rule_snapshot:{}},allocations:[],refunds:[]}; }
   if(command==='prepare_write') {
     if(args.operation!=='create_account' || typeof args.body!=='string' || args.target!==null) throw 'wrong prepare shape';
     return {id:write,key:'same-idempotency-key',path:'accounts'};
   }
   if(command==='execute_write') {
     if(args.request!==write || Object.keys(args).length!==2) throw 'request must use opaque handle';
     if(++attempts===1) throw {Api:{status:503,code:'retryable',message:'测试：结果未知'}};
     return {done:true};
   }
   if(command==='discard_write' || command==='session_logout') return null;
   throw 'unexpected command';
 }}};
})();
'''
try:
    with sync_playwright() as p:
        executable = os.environ.get("CONSOLE_BROWSER") or shutil.which("google-chrome") or shutil.which("chromium")
        browser = p.chromium.launch(executable_path=executable, headless=True, args=["--no-sandbox"])
        page = browser.new_page(viewport={"width": 1280, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.add_init_script(stub)
        page.goto(f"http://127.0.0.1:{server.server_port}", wait_until="networkidle")
        expect(page.get_by_role("button", name="安全登录")).to_be_visible(timeout=15000)
        expect(page.locator("#api-origin")).to_be_enabled()
        page.locator("#api-origin").fill("https://fixture.invalid")
        page.locator("input[type=password]").fill("fixture-only")
        page.get_by_role("button", name="安全登录").click()
        expect(page.get_by_text("IPC测试管理员", exact=True)).to_be_visible()
        page.locator("nav").get_by_role("button", name="订单管理", exact=True).click()
        expect(page.get_by_role("button", name="查看详情", exact=True)).to_be_visible()
        page.get_by_role("button", name="查看详情", exact=True).click()
        expect(page.get_by_role("heading", name="订单详情", exact=True)).to_be_visible()
        expect(page.locator("article.metric").filter(has_text="实付金额").locator("strong")).to_have_text("¥ 100.00")
        page.locator("nav").get_by_role("button", name="佣金试算", exact=True).click()
        panel = page.locator("section.panel").first
        panel.locator("input").nth(0).fill("11111111-1111-4111-8111-111111111111")
        page.get_by_role("button", name="向服务器试算").click()
        expect(panel.locator("pre")).to_contain_text("binding")
        op = page.locator("section.operations")
        op.get_by_role("button", name="校验并准备请求").click()
        expect(op.locator("textarea")).to_be_disabled()
        expect(op.get_by_role("button", name="确认提交", exact=True)).to_be_disabled()
        op.locator("input[type=checkbox]").check()
        op.get_by_role("button", name="确认提交", exact=True).click()
        expect(op.get_by_role("button", name="以原幂等键重试")).to_be_visible()
        expect(page.get_by_role("button", name="退出并清除会话")).to_be_disabled()
        expect(op.get_by_role("button", name="返回编辑 / 新操作")).to_be_disabled()
        op.get_by_role("button", name="以原幂等键重试").click()
        expect(op.get_by_role("button", name="返回编辑 / 新操作")).to_be_enabled()
        op.get_by_role("button", name="返回编辑 / 新操作").click()
        expect(op.locator("textarea")).to_be_enabled()
        page.get_by_role("button", name="退出并清除会话").click()
        expect(page.get_by_role("button", name="安全登录")).to_be_visible()
        calls = page.evaluate("window.fixtureCalls")
        methods = [c["command"] for c in calls]
        assert set(methods) == {"session_login", "session_logout", "read_resource", "quote_commission", "order_detail", "prepare_write", "execute_write", "discard_write"}, methods
        executes = [c for c in calls if c["command"] == "execute_write"]
        assert len(executes) == 2 and executes[0] == executes[1]
        assert all("token" not in c["args"] for c in calls[1:])
        assert not errors, errors
        result = {"tested_ref": os.environ.get("GITHUB_SHA", "local"), "mode": "real tauri-feature release WASM with IPC fixtures, NOT Tauri runtime/E2E", "calls": len(calls), "methods": sorted(set(methods)), "errors": errors, "passed": True}
        (OUT / "tauri-transport-result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False, indent=2))
        browser.close()
finally:
    server.shutdown()
