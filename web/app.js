'use strict';

const $ = (id) => document.getElementById(id);
const state = { token: '', actor: null, view: 'overview', offset: 0, accounts: [], pending: null };
const roles = { admin:'管理员', operator:'运营', finance:'财务', integrator:'业务接入', auditor:'审计', member:'账户成员' };
const kinds = { platform:'平台', clearing:'清算', merchant:'商家', promoter:'推广员' };
const slots = { direct:'一级返佣', indirect:'二级返佣', platform:'平台净佣金', merchant:'商家货款' };
const buckets = { frozen:'冻结余额', available:'可用余额', reserved:'提现占用' };
const events = { capture:'订单入账', refund:'退款冲正', release:'到期解冻', payout_reserve:'提现占用', payout_reject:'驳回释放', payout_paid:'出款确认', payout_failed:'失败释放' };
const statuses = { requested:'待审核', approved:'已审核', processing:'执行中', unknown:'结果未知', succeeded:'已成功', failed:'已失败', rejected:'已驳回' };
const views = [
  ['overview','总览','◈','OVERVIEW','资金与业务总览','从订单到账本，让每一次资金变动可追溯。',null],
  ['accounts','账户管理','▦','ACCOUNTS','商家与推广员','维护结算主体；推广员上级关系创建后不可更改。',null],
  ['referrals','推广关系','◇','REFERRALS','客户推广归属','客户首单入账前绑定；已有订单后不可补绑。',['operator','integrator','auditor']],
  ['rules','抽佣规则','≋','COMMISSION RULES','规则与版本','商家规则优先于全局规则；同范围按优先级、版本选择。',['operator','integrator','finance','auditor']],
  ['orders','订单与退款','▤','ORDERS','订单与退款','接收已核实的支付成功事实，不在此页面发起收款或退款。',['operator','integrator','finance','auditor']],
  ['commissions','分配明细','⌗','ALLOCATIONS','佣金与货款分配','保留原始分配、累计退回与当前净额。',null],
  ['wallets','账户余额','▣','BALANCES','余额与追偿','冻结、可用和提现占用分开记账；负可用余额表示待追偿。',null],
  ['payouts','提现结算','↗','SETTLEMENTS','提现与结算','申请、审核、执行、核验分离；未知结果不会释放占用。',null],
  ['ledger','账本流水','≡','LEDGER','不可变账本流水','历史分录不覆盖；退款与结算均追加新分录。',null],
  ['reconciliation','内部对账','⊙','RECONCILIATION','账务一致性检查','核对分录平衡、余额投影、订单分配和提现占用。',['finance','auditor']],
  ['audit','审计日志','◷','AUDIT TRAIL','操作审计','记录操作者、业务变更和对应结果，不记录访问令牌明文。',['operator','finance','auditor']],
  ['outbox','业务事件','⇄','OUTBOX','可靠事件队列','按租约领取、处理后确认；消费者按事件 ID 去重。',['integrator','auditor']],
  ['credentials','访问凭据','⚿','ACCESS','访问令牌与权限','为运营、财务、业务系统和账户成员分别签发独立令牌。',[]],
];

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (value !== null && value !== undefined) node.setAttribute(key, String(value));
  }
  for (const child of children.flat()) {
    if (child !== null && child !== undefined) node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}
function can(...allowed) { return state.actor?.role === 'admin' || allowed.includes(state.actor?.role); }
function money(value = '0') {
  try { const n = BigInt(value); const a = n < 0n ? -n : n; return `${n < 0n ? '-' : ''}¥${(a / 100n).toLocaleString('zh-CN')}.${(a % 100n).toString().padStart(2,'0')}`; }
  catch { return '—'; }
}
function minor(yuan) {
  const value = String(yuan).trim();
  if (!/^\d+(\.\d{1,2})?$/.test(value)) throw new Error('金额须为非负数，最多两位小数。');
  const [whole, fraction=''] = value.split('.');
  const result = BigInt(whole) * 100n + BigInt(fraction.padEnd(2,'0'));
  if (result > 100000000000000n) throw new Error('单次金额超出系统上限。');
  return result.toString();
}
function date(value) { return value ? new Date(value).toLocaleString('zh-CN',{hour12:false,timeZone:'Asia/Shanghai'}) : '—'; }
function short(value) { return value ? `${String(value).slice(0,8)}…` : '—'; }
function badge(text, color='') { return el('span',{class:`badge ${color}`,text}); }
function status(value) { return badge(statuses[value] || value, value==='succeeded'?'green':value==='unknown'?'red':['requested','approved','processing'].includes(value)?'orange':''); }
function flash(message, failure=false) { $('flash').hidden=false; $('flash').className=`flash ${failure?'failure':''}`; $('flash').textContent=message; }
function button(text, callback, cls='') { return el('button',{type:'button',class:cls,onclick:callback,text}); }

async function api(path, {method='GET', body, idempotencyKey} = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 20000);
  const headers = {Authorization:`Bearer ${state.token}`};
  if (body !== undefined) headers['Content-Type']='application/json';
  if (idempotencyKey) headers['Idempotency-Key']=idempotencyKey;
  try {
    const response = await fetch(`/api/v1${path}`,{method,headers,body:body===undefined?undefined:JSON.stringify(body),signal:controller.signal,cache:'no-store'});
    const result = await response.json();
    if (!response.ok) {
      const error = new Error(result.error?.message || `请求失败：${response.status}`);
      error.uncertain = response.status >= 500;
      error.status = response.status;
      throw error;
    }
    return result;
  } catch(error) {
    if (!error.status) error.uncertain=true;
    throw error;
  } finally { clearTimeout(timer); }
}

$('login-form').addEventListener('submit',async(event)=>{
  event.preventDefault(); $('login-error').textContent='';
  state.token=$('token').value.trim();
  const submit=$('login-form').querySelector('button'); submit.disabled=true;
  try {
    state.actor=await api('/me'); $('token').value='';
    $('login').hidden=true; $('app').hidden=false;
    $('user-name').textContent=state.actor.name; $('user-role').textContent=roles[state.actor.role] || state.actor.role;
    renderNav(); await navigate('overview');
  } catch(error) { state.token=''; $('login-error').textContent=error.message; }
  finally { submit.disabled=false; }
});
$('logout').addEventListener('click',()=>{
  state.token=''; state.actor=null; state.accounts=[]; state.pending=null;
  $('modal').close(); $('modal-content').replaceChildren(); $('content').replaceChildren();
  $('app').hidden=true; $('login').hidden=false; $('token').value='';
});

function renderNav() {
  $('nav').replaceChildren(...views.filter(v=>v[6]===null || can(...v[6])).map(v=>
    el('button',{type:'button','data-view':v[0],'aria-label':v[1],title:v[1],onclick:()=>navigate(v[0])},el('span',{class:'nav-icon',text:v[2]}),el('span',{class:'nav-text',text:v[1]}))));
}
async function navigate(view, offset=0) {
  state.view=view; state.offset=offset; $('flash').hidden=true;
  const config=views.find(v=>v[0]===view);
  if (!config) return;
  $('breadcrumb').textContent=config[1]; $('page-eyebrow').textContent=config[3]; $('page-title').textContent=config[4]; $('page-description').textContent=config[5];
  document.querySelectorAll('nav button').forEach(b=>b.classList.toggle('active',b.dataset.view===view));
  $('page-actions').replaceChildren(button('刷新',()=>navigate(view,state.offset)));
  configureActions(view);
  $('content').replaceChildren(el('div',{class:'loading',text:'正在读取当前账务数据…'}));
  try {
    if (view==='overview') return await overview();
    if (view==='reconciliation') return await reconciliation();
    const result=await api(`/${view}?limit=30&offset=${offset}`);
    if (state.view!==view || state.offset!==offset) return;
    renderTable(view,result);
  } catch(error) {
    $('content').replaceChildren(el('div',{class:'panel empty'},el('strong',{text:'读取未完成'}),el('p',{text:error.message})));
    if (error.status===401) flash('令牌可能已过期或撤销，请退出后重新登录。',true);
  }
}

function configureActions(view) {
  const actions=$('page-actions');
  if (view==='accounts' && can('operator')) actions.prepend(button('＋ 新建账户',accountForm,'primary'));
  if (view==='referrals' && can('operator','integrator')) actions.prepend(button('＋ 绑定推广关系',referralForm,'primary'));
  if (view==='rules' && can('operator')) actions.prepend(button('＋ 新建规则版本',ruleForm,'primary'));
  if (view==='orders' && can('operator','integrator')) actions.prepend(button('＋ 录入已支付订单',orderForm,'primary'));
  if (view==='payouts' && can('operator','member')) actions.prepend(button('＋ 申请提现',payoutForm,'primary'));
  if (view==='credentials' && can()) actions.prepend(button('＋ 签发令牌',credentialForm,'primary'));
}
function metric(label,value,note='',emphasis=false) { return el('div',{class:`metric ${emphasis?'emphasis':''}`},el('div',{class:'metric-label',text:label}),el('div',{class:'metric-value',text:value}),el('div',{class:'metric-note',text:note})); }
function panel(title,...children) { return el('section',{class:'panel'},el('div',{class:'panel-header'},el('h3',{text:title})),el('div',{class:'panel-body'},children)); }
async function overview() {
  const data=await api('/dashboard'); if(state.view!=='overview') return;
  const cards=el('div',{class:'cards'});
  if(data.scope==='platform') cards.append(metric('累计入账金额',money(data.paid_minor),`${data.order_count} 笔订单`,true),metric('平台净佣金',money(data.platform_net_minor),'原始平台佣金减累计退佣'),metric('累计退款',money(data.refunded_minor),'仅记录已核实的退款事实'),metric('待审核 / 执行',String(data.pending_payouts),'笔提现单'));
  cards.append(metric('冻结余额',money(data.frozen_minor),'达到冻结期限后解冻'),metric('可用余额',money(data.available_minor),'负值表示净追偿余额'),metric('提现占用',money(data.reserved_minor),'申请时占用，核验后释放'),metric('待追偿金额',money(data.debt_minor),'不含支付清算对手账户'));
  const workflow=el('div',{class:'workflow'},...[
    ['交易入账','锁定本次规则及推广关系，计算商家货款和各方佣金。'],['冻结与解冻','冻结期内退款从冻结余额冲减；期满后转入可用。'],['审核与结算','独立凭据审核，先登记执行，再核验外部付款结果。'],['退款与追偿','已出款后退款形成明确欠款，由后续收入抵扣。'],
  ].map((v,i)=>el('div',{},el('span',{class:'step-number',text:`0${i+1}`}),el('h3',{text:v[0]}),el('p',{text:v[1]}))));
  const nodes=[cards];
  if(BigInt(data.debt_minor || '0')>0n || Number(data.unknown_payouts || 0)>0) nodes.push(el('div',{class:'attention'},el('strong',{text:'需要财务跟进'}),el('span',{text:`待追偿 ${money(data.debt_minor)}；结果未知 ${data.unknown_payouts || 0} 笔。未知结果不可当作失败重付。`})));
  nodes.push(panel('账务处理流程',workflow));
  nodes.push(el('div',{class:'grid-two'},panel('运行概况',...[
    ['系统模式','单平台 / 多商家 / CNY'],['待解冻订单',data.due_orders ?? '仅运营可见'],['待投递业务事件',data.outbox_pending ?? '仅运营可见'],['当前权限',roles[state.actor.role]],
  ].map(v=>el('div',{class:'kv'},el('span',{text:v[0]}),el('strong',{text:String(v[1])})))),panel('使用边界',el('p',{class:'muted',text:'本版本管理佣金业务账，不提供收银台、不直接发起银行转账。支付和退款事实由可信业务系统接入。'}),el('p',{class:'muted',text:'内部一致性检查不替代支付机构账单核对。正式出款前须完成支付渠道接入、安全审计与业务验收。'}))));
  $('content').replaceChildren(...nodes);
}

const schema = {
  accounts:[['账户名称','name'],['业务编号','external_id'],['类型',r=>badge(kinds[r.kind] || r.kind)],['上级推广员',r=>short(r.parent_id)],['状态',r=>badge(r.active?'启用':'停用',r.active?'green':'')],['创建时间',r=>date(r.created_at)]],
  referrals:[['客户业务编号','customer_external_id'],['推广员','promoter_name'],['推广员 ID',r=>short(r.promoter_id)],['绑定时间',r=>date(r.created_at)]],
  rules:[['规则名称','name'],['版本',r=>`v${r.version}`],['适用范围',r=>r.merchant_id?short(r.merchant_id):'全局'],['平台费率',r=>`${r.rate_bps / 100}%`],['固定费用',r=>money(r.fixed_minor)],['一级 / 二级',r=>`${r.direct_bps / 100}% / ${r.indirect_bps / 100}%`],['冻结时间',r=>`${r.freeze_seconds} 秒`],['状态',r=>badge(r.active?'启用':'停用',r.active?'green':'')]],
  orders:[['订单编号','external_id'],['实付金额',r=>money(r.paid_minor)],['计佣基数',r=>money(r.commission_base_minor)],['佣金池',r=>money(r.fee_pool_minor)],['累计退款',r=>money(r.refunded_minor)],['资金状态',r=>badge(r.released_at?'已解冻':'冻结中',r.released_at?'green':'orange')],['入账时间',r=>date(r.captured_at)]],
  commissions:[['订单编号','external_id'],['账户','name'],['分配类型',r=>slots[r.slot]],['原始金额',r=>money(r.original_minor)],['累计退回',r=>money(r.refunded_minor)],['净额',r=>money(r.net_minor)],['状态',r=>badge(r.released_at?'可用 / 已结算':'冻结中',r.released_at?'green':'orange')]],
  wallets:[['账户','name'],['类型',r=>kinds[r.kind]],['冻结余额',r=>money(r.frozen_minor)],['可用余额',r=>el('span',{class:`money ${BigInt(r.available_minor)<0n?'negative':''}`,text:money(r.available_minor)})],['提现占用',r=>money(r.reserved_minor)],['待追偿',r=>money(r.debt_minor)],['更新时间',r=>date(r.updated_at)]],
  payouts:[['业务编号','external_id'],['账户 ID',r=>short(r.account_id)],['提现金额',r=>money(r.amount_minor)],['状态',r=>status(r.status)],['收款方引用','destination_ref'],['外部流水',r=>r.provider_reference || '—'],['申请时间',r=>date(r.created_at)]],
  ledger:[['业务类型',r=>events[r.kind] || r.kind],['账户','name'],['余额分类',r=>buckets[r.bucket]],['变动金额',r=>el('span',{class:`money ${BigInt(r.delta_minor)<0n?'negative':''}`,text:money(r.delta_minor)})],['业务事件','event_key'],['记账时间',r=>date(r.created_at)]],
  audit:[['审计序号',r=>String(r.id)],['操作','action'],['操作者',r=>r.actor_id?short(r.actor_id):'系统任务'],['目标对象',r=>short(r.target_id)],['发生时间',r=>date(r.created_at)]],
  credentials:[['凭据名称','name'],['角色',r=>badge(roles[r.role])],['账户 ID',r=>short(r.account_id)],['到期时间',r=>date(r.expires_at)],['状态',r=>badge(r.revoked_at?'已撤销':new Date(r.expires_at)<new Date()?'已过期':'有效',r.revoked_at?'':'green')]],
  outbox:[['事件 ID',r=>short(r.id)],['主题','topic'],['领取次数',r=>String(r.attempts)],['投递状态',r=>badge(r.delivered_at?'已确认':'待确认',r.delivered_at?'green':'orange')],['租约到期',r=>date(r.lease_until)],['生成时间',r=>date(r.created_at)]],
};
function renderTable(view,data) {
  const columns=schema[view];
  const box=el('section',{class:'panel'});
  if(!data.items.length) box.append(el('div',{class:'empty'},el('strong',{text:'暂无记录'}),el('p',{text:'通过右上角操作或 API 接入产生真实业务数据。'})));
  else {
    const table=el('table',{},el('thead',{},el('tr',{},...columns.map(c=>el('th',{text:c[0]})),el('th',{text:'操作'}))));
    const tbody=el('tbody');
    data.items.forEach(row=>{
      const cells=columns.map(([,field])=>{const value=typeof field==='function'?field(row):row[field];return el('td',{},value??'—');});
      cells.push(el('td',{},rowActions(view,row))); tbody.append(el('tr',{},cells));
    });
    table.append(tbody); box.append(el('div',{class:'table-wrap'},table));
  }
  const previous=button('上一页',()=>navigate(view,Math.max(0,state.offset-30)));previous.disabled=data.offset===0;
  const next=button('下一页',()=>navigate(view,state.offset+30));next.disabled=!data.has_more;
  box.append(el('div',{class:'pagination'},el('span',{text:`第 ${Math.floor(data.offset/30)+1} 页 · 本页 ${data.items.length} 条`}),previous,next));
  $('content').replaceChildren(box);
}
function rowActions(view,row) {
  const actions=el('div',{class:'row-actions'},button('详情',async()=>{
    try { detail('记录详情',view==='orders'?await api(`/orders/${row.id}`):row); }catch(e){flash(e.message,true);}
  },'small-button'));
  if(view==='rules' && row.active && can('operator')) actions.append(button('停用',()=>simpleAction('停用此规则',`/rules/${row.id}/disable`,'只影响后续入账；历史订单仍使用原规则快照。'),'small-button danger'));
  if(view==='orders') {
    if(can('operator','integrator') && BigInt(row.refunded_minor)<BigInt(row.paid_minor)) actions.append(button('退款记账',()=>refundForm(row),'small-button'));
    if(can('operator','finance') && !row.released_at) actions.append(button('到期解冻',()=>simpleAction('解冻订单',`/orders/${row.id}/release`,'服务端会检查冻结期限，不允许强制提前解冻。'),'small-button'));
  }
  if(view==='payouts' && can('finance')) {
    if(row.status==='requested') actions.append(button('审核通过',()=>simpleAction('审核提现',`/payouts/${row.id}/approve`,'申请凭据不能审核自己的提现。请核验账户和收款方引用。'),'small-button'));
    if(['requested','approved'].includes(row.status)) actions.append(button('驳回',()=>reasonAction('驳回提现',`/payouts/${row.id}/reject`),'small-button danger'));
    if(row.status==='approved') actions.append(button('开始执行',()=>reasonAction('登记开始执行',`/payouts/${row.id}/processing`,'这不会自动转账。登记后，以提现 ID 作为外部渠道幂等键；遇到结果未知不得换号重付。'),'small-button'));
    if(['processing','unknown'].includes(row.status)) actions.append(button('核验结果',()=>outcomeForm(row),'small-button'));
  }
  if(view==='credentials' && !row.revoked_at && can()) actions.append(button('撤销',()=>simpleAction('撤销令牌',`/credentials/${row.id}/revoke`,'撤销后立即失效；不能撤销最后一个有效管理员令牌。'),'small-button danger'));
  return actions;
}

function modalHeader(title) { return el('div',{class:'dialog-header'},el('h2',{text:title}),button('关闭',()=>closeModal(),'text-button')); }
function closeModal() {
  if (state.pending?.inFlight) return; // Preserve the request key until its outcome is known.
  if(state.pending?.uncertain && !confirm(`上次写操作结果未知。幂等键：${state.pending.key}。关闭前请保存此键并核对业务记录。确认关闭？`)) return;
  $('modal').close(); $('modal-content').replaceChildren(); state.pending=null;
}
$('modal').addEventListener('cancel',event=>{event.preventDefault();closeModal();});
function detail(title,data) { state.pending=null; $('modal-content').replaceChildren(modalHeader(title),el('div',{class:'dialog-body'},el('pre',{text:JSON.stringify(data,null,2)}))); if(!$('modal').open)$('modal').showModal(); }
function field(config) {
  const {name,label,type='text',value='',options,help,required=false,wide=false}=config;
  let input;
  if(options) input=el('select',{name,id:`field-${name}`},...options.map(([v,l])=>el('option',{value:v,text:l})));
  else if(type==='textarea') input=el('textarea',{name,id:`field-${name}`});
  else input=el('input',{type,name,id:`field-${name}`,autocomplete:'off',spellcheck:'false'});
  if (!options || config.value !== undefined) input.value=String(value); input.required=required;
  if(type==='number'){input.min=config.min??'0';input.step=config.step??'1';}
  if(config.readonly)input.readOnly=true;
  const wrapper=el('div',{class:`field ${wide?'wide-field':''}`},el('label',{for:`field-${name}`,text:label}),input);
  if(help)wrapper.append(el('p',{text:help}));
  return wrapper;
}
function formModal({title,note,fields,path,build,preview}) {
  state.pending=null;
  const form=el('form'); const errorBox=el('p',{class:'error form-error',role:'alert'});
  const body=el('div',{class:'dialog-body'});if(note)body.append(el('div',{class:'notice',text:note}));
  body.append(el('div',{class:'form-grid'},fields.map(field)));
  const previewBox=el('pre',{hidden:'hidden'});body.append(previewBox);
  const submit=el('button',{type:'submit',class:'primary',text:'确认提交'});
  const controls=el('div',{class:'dialog-footer'},button('取消',closeModal),submit);
  if(preview)controls.prepend(button('试算',async()=>{try{const payload=build(Object.fromEntries(new FormData(form)));const result=await preview(payload);previewBox.hidden=false;previewBox.textContent=JSON.stringify(result,null,2);}catch(error){errorBox.textContent=error.message;}}));
  form.append(body,errorBox,controls);
  form.addEventListener('submit',async event=>{
    event.preventDefault();errorBox.textContent='';submit.disabled=true;
    try {
      let payload;
      if(state.pending?.uncertain)payload=state.pending.body;
      else payload=build(Object.fromEntries(new FormData(form)));
      if(!state.pending)state.pending={key:crypto.randomUUID(),body:payload,uncertain:false};
      state.pending.body=payload;
      state.pending.inFlight=true;
      await api(path,{method:'POST',body:payload,idempotencyKey:state.pending.key});
      state.pending=null;$('modal').close();$('modal-content').replaceChildren();await navigate(state.view,state.offset);flash('操作已提交并完成事务记账。');
    } catch(error) {
      if(state.pending)state.pending.inFlight=false;
      if(state.pending && error.uncertain){
        state.pending.uncertain=true;
        form.querySelectorAll('input,select,textarea').forEach(input=>{input.disabled=true;});
        errorBox.textContent=`结果未知，不能判断是否已提交。请点击重试，复用原请求与幂等键：${state.pending.key}\n${error.message}`;
        submit.textContent='按原幂等键重试';
      }else{
        state.pending=null;errorBox.textContent=error.message;submit.textContent='确认提交';
        form.querySelectorAll('input,select,textarea').forEach(input=>{input.disabled=false;});
      }
    }finally{submit.disabled=false;}
  });
  $('modal-content').replaceChildren(modalHeader(title),form);if(!$('modal').open)$('modal').showModal();
}
function simpleAction(title,path,note) { formModal({title,path,note,fields:[],build:()=>({})}); }
function reasonAction(title,path,note='请填写处理原因；该记录将进入不可修改的审计日志。') {formModal({title,path,note,fields:[{name:'reason',label:'原因 / 执行说明',type:'textarea',required:true,wide:true}],build:d=>({reason:d.reason})});}
async function loadAccounts() {
  try { state.accounts=(await api('/accounts?limit=200')).items; return state.accounts; }
  catch(error){flash(error.message,true);return null;}
}
function accountOptions(kind,optional=false) {
  const rows=state.accounts.filter(a=>(!kind || a.kind===kind) && a.kind!=='clearing' && a.active);
  return [...(optional?[['','不设置']]:[]),...rows.map(a=>[a.id,`${a.name} · ${a.external_id}`])];
}
async function accountForm() {
  if(!await loadAccounts())return;
  formModal({title:'新建账户',path:'/accounts',note:'上级仅用于推广员。上级关系创建后不可更改，避免历史归属漂移。',fields:[
    {name:'name',label:'账户名称',required:true},{name:'external_id',label:'业务编号',required:true},
    {name:'kind',label:'账户类型',options:[['merchant','商家'],['promoter','推广员']],value:'merchant'},
    {name:'parent_id',label:'上级推广员（可选）',options:accountOptions('promoter',true)},
  ],build:d=>({name:d.name,external_id:d.external_id,kind:d.kind,parent_id:d.parent_id||null})});
}
async function referralForm() {
  if(!await loadAccounts())return;
  formModal({title:'绑定客户推广归属',path:'/referrals',fields:[{name:'customer_external_id',label:'客户业务编号',required:true},{name:'promoter_id',label:'一级推广员',options:accountOptions('promoter'),required:true}],build:d=>d});
}
async function ruleForm() {
  if(!await loadAccounts())return;
  formModal({title:'新建规则版本',path:'/rules',note:'返佣比例从平台佣金池内分配，不在商家实付上额外叠加。100 基点 = 1%。费率变更请新增版本。',fields:[
    {name:'name',label:'规则名称',required:true},{name:'merchant_id',label:'适用商家（空为全局）',options:accountOptions('merchant',true)},
    {name:'rate_bps',label:'平台抽佣费率（基点）',type:'number',value:'1000',required:true},{name:'fixed',label:'固定费用（元）',value:'0.00',required:true},
    {name:'cap',label:'佣金池封顶（元，可选）'},{name:'priority',label:'优先级（较大优先）',type:'number',min:'-10000',value:'0'},
    {name:'direct_bps',label:'一级返佣 / 佣金池（基点）',type:'number',value:'3000'},{name:'indirect_bps',label:'二级返佣 / 佣金池（基点）',type:'number',value:'1000'},
    {name:'freeze_days',label:'冻结天数',type:'number',value:'7',help:'可设 0 用于联调；实际解冻由任务或到期操作执行。'},
    {name:'min_base',label:'基数下界（元，包含）',value:'0.00'},{name:'max_base',label:'基数上界（元，不包含，可选）'},
    {name:'effective_from',label:'生效时间（可选）',type:'datetime-local'},{name:'effective_until',label:'失效时间（可选）',type:'datetime-local'},
  ],build:d=>({name:d.name,merchant_id:d.merchant_id||null,priority:Number(d.priority),min_base_minor:minor(d.min_base),max_base_minor:d.max_base?minor(d.max_base):null,
    effective_from:d.effective_from?new Date(d.effective_from).toISOString():null,effective_until:d.effective_until?new Date(d.effective_until).toISOString():null,
    terms:{rate_bps:Number(d.rate_bps),fixed_minor:minor(d.fixed),cap_minor:d.cap?minor(d.cap):null,direct_bps:Number(d.direct_bps),indirect_bps:Number(d.indirect_bps),freeze_seconds:Number(d.freeze_days)*86400}})});
}
async function orderForm() {
  if(!await loadAccounts())return;
  formModal({title:'录入已支付订单',path:'/orders',note:'仅用于可信业务系统已核实的支付成功事实。该操作不收款；同一业务订单号不允许重复入账。',fields:[
    {name:'external_id',label:'订单业务编号',required:true},{name:'merchant_id',label:'商家',options:accountOptions('merchant'),required:true},
    {name:'customer_external_id',label:'客户业务编号（可选）',help:'有推广关系时会自动计算两级返佣。'},{name:'paid',label:'实付金额（元）',required:true},
    {name:'base',label:'计佣基数（元）',required:true,help:'不超过实付金额；是否剔除运费等由上游确定。'},
  ],build:d=>({external_id:d.external_id,merchant_id:d.merchant_id,customer_external_id:d.customer_external_id||null,currency:'CNY',paid_minor:minor(d.paid),commission_base_minor:minor(d.base)}),
  preview:p=>api('/quotes',{method:'POST',body:{merchant_id:p.merchant_id,customer_external_id:p.customer_external_id,paid_minor:p.paid_minor,commission_base_minor:p.commission_base_minor}})});
}
function refundForm(row) {
  formModal({title:'登记已核实退款',path:`/orders/${row.id}/refunds`,note:`订单 ${row.external_id}，尚可退款 ${money((BigInt(row.paid_minor)-BigInt(row.refunded_minor)).toString())}。这不会调用支付渠道退款。已提现后的退佣可能形成欠款。`,fields:[
    {name:'external_id',label:'退款业务编号',value:`R-${crypto.randomUUID()}`,required:true,wide:true},{name:'amount',label:'本次退款（元）',required:true},
    {name:'reason',label:'原因 / 外部退款凭据',type:'textarea',required:true,wide:true},
  ],build:d=>({external_id:d.external_id,amount_minor:minor(d.amount),reason:d.reason})});
}
async function payoutForm() {
  if(!await loadAccounts())return;
  formModal({title:'申请提现 / 结算',path:'/payouts',note:'申请后立即占用可用余额。收款方引用须由支付系统或受控台账管理，不要填写银行卡明文。',fields:[
    {name:'external_id',label:'提现业务编号',value:`P-${crypto.randomUUID()}`,required:true,wide:true},{name:'account_id',label:'结算账户',options:accountOptions(),required:true},
    {name:'amount',label:'提现金额（元）',required:true},{name:'destination_ref',label:'已核验收款方引用',required:true,wide:true},
  ],build:d=>({external_id:d.external_id,account_id:d.account_id,amount_minor:minor(d.amount),destination_ref:d.destination_ref})});
}
function outcomeForm(row) {
  formModal({title:'核验外部出款结果',path:`/payouts/${row.id}/outcome`,note:'请以银行或支付机构的最终状态为准。连接超时、回调缺失或查询失败应选择“结果未知”，不能直接认定失败。',fields:[
    {name:'status',label:'核验结果',options:[['unknown','结果未知：保留占用'],['succeeded','已成功：确认出款'],['failed','明确失败：返还占用']],value:'unknown'},
    {name:'provider_reference',label:'外部唯一交易流水号（成功必填）'},
    {name:'evidence',label:'核验凭据 / 说明',type:'textarea',required:true,wide:true},
  ],build:d=>({status:d.status,provider_reference:d.provider_reference||null,evidence:d.evidence})});
}
async function credentialForm() {
  if(!await loadAccounts())return;
  const bytes=crypto.getRandomValues(new Uint8Array(32));const secret=`cms_${Array.from(bytes,b=>b.toString(16).padStart(2,'0')).join('')}`;
  formModal({title:'签发独立访问令牌',path:'/credentials',note:'请先复制并安全保管下面的令牌；数据库只保存摘要，提交后不会再次展示明文。审核与申请须使用不同凭据，实际应交由不同人员持有。',fields:[
    {name:'name',label:'凭据名称',required:true},{name:'role',label:'角色',options:Object.entries(roles),value:'operator'},
    {name:'account_id',label:'关联账户（仅成员角色）',options:accountOptions(undefined,true)},{name:'expires_in_days',label:'有效期（天）',type:'number',min:'1',value:'30'},
    {name:'secret',label:'访问令牌（仅本次展示）',value:secret,readonly:true,wide:true},
  ],build:d=>({name:d.name,role:d.role,account_id:d.role==='member'?(d.account_id||null):null,expires_in_days:Number(d.expires_in_days),secret:d.secret})});
}
async function reconciliation() {
  const result=await api('/reconciliation');if(state.view!=='reconciliation')return;
  $('content').replaceChildren(el('div',{class:`panel reconcile-result ${result.ok?'':'failure'}`},el('h2',{text:result.ok?'内部账务检查一致':'发现内部账务差异'}),
    el('p',{class:'muted',text:'本次使用一致性数据库快照检查。结果不包含外部支付机构、银行账单或资金托管核对。'}),el('pre',{text:JSON.stringify(result,null,2)})));
}
