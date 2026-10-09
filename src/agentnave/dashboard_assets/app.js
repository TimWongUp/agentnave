"use strict";
const $ = id => document.getElementById(id);
const labels = {held:"已收存 · 尚未揭晓",idle:"等待主持人开场",running:"正在组织回应",ready:"等待导演发布",failed:"本轮未完成，等待导演处理",cancelled:"本轮已取消",interrupted:"上次运行中断，等待导演继续",published:"台词已发布",discarded:"导演已收回本轮候选台词"};
const host = {id:"director",label:"主持人",model:"导演扮演",provider:"host"};
const modes={discussion:"一起聊",blind:"各自答",task:"做任务"};
const taskStates={running:"进行中",succeeded:"已回复 · 待验收",failed:"失败",blocked:"需处理",cancelled:"已取消",interrupted:"已中断 · 需处理"};
let current = null, selected = null, messageKey = "", seatKey = "", previousCount = 0;
let homeKey = "", taskKey = "", heldKey = "", modeFilter = "all", privateView = false;
function el(tag, className, text) { const node=document.createElement(tag); node.className=className; if(text!==undefined)node.textContent=text; return node; }
function avatar(seat) {
  const key=(seat.model+" "+seat.provider).toLowerCase();
  const kind=seat.id==="director"?"host":["claude","gpt","gemini","grok","deepseek"].find(name=>key.includes(name));
  const node=el("span","avatar avatar-"+(kind||"unknown"),kind?undefined:seat.label.slice(0,1));
  node.setAttribute("aria-hidden","true");return node;
}
function choose(id) { selected=id; seatKey="";messageKey="";if(current)render(current); }
function render(data) {
  const nearBottom=window.innerHeight+window.scrollY>=document.documentElement.scrollHeight-160;
  const hadData=current!==null;current=data;
  const isHome=data.view==="workbench", isTask=data.mode==="task";
  document.body.classList.toggle("home-view",isHome);document.body.classList.toggle("task-view",isTask);
  $("conversation-list").hidden=!isHome;$("task-list").hidden=!isTask;
  $("workbench-link").hidden=!data.workbench_url;$("workbench-link").href=data.workbench_url||"./";
  $("home-tools").hidden=!isHome;
  $("storage-details").hidden=!data.storage;$("storage-note").textContent=data.storage||"";
  $("brand-link").href=data.workbench_url||"./";
  document.querySelector(".brand-sub").textContent=data.view==="public"?"公共对话":"工作台";
  $("footer-note").textContent=isHome?"本地工作台 · 操作由主控 Agent 执行":isTask?"任务结果 · 仅主控可见":data.view==="director"?"主控工作台 · 网页只读":"公开对话 · 网页只读";
  if(isHome){
    document.title="AgentNave · 工作台";$("title").textContent="所有会话";$("view-name").textContent="我的工作台";
    renderHome(data);return;
  }
  const director=data.view==="director", cast=[host,...data.seats];
  $("mode-note").textContent=data.mode==="blind"?"各自答 · 收齐后统一揭晓":isTask?"做任务 · 仅主控可见":"一起聊 · 逐条公开";
  $("title").textContent=data.title;document.title=data.title+" · "+(director?"工作台":"对话区");
  $("view-name").textContent=isTask?"做任务 · 私有会话":director?"节目会话 · 主控视角":"公共对话 · 全员可见";
  $("view-note").textContent="选手和主持人的公开发言";
  $("view-description").textContent=data.mode==="blind"?"未揭晓的答案只在导演后台可见，收齐后统一公开。":"这里与选手收到的公开记录同步，筛选只影响当前页面。";
  $("view-tabs").hidden=!director||isTask;
  $("director-panel").hidden=!director||isTask||!privateView;
  $("transcript-panel").hidden=isTask||(director&&privateView);
  $("conversation-tab").setAttribute("aria-pressed",String(!privateView));
  $("backstage-tab").setAttribute("aria-pressed",String(privateView));
  $("public-link").hidden=!director||isTask;
  document.querySelector(".export").hidden=isTask||(director&&privateView);
  if(director){$("public-link").href=data.public_url;$("instruction").textContent=data.instruction||"尚未下达指令";$("candidate").textContent=data.status==="ready"?data.candidate:"当前没有待发布回答";$("turn-error").textContent=data.error?"本轮提示："+data.error:"";}
  $("held-panel").hidden=!director||data.mode!=="blind";
  $("candidate-panel").hidden=data.mode==="blind";
  const nextHeldKey=JSON.stringify([data.pending_answers,data.absent,data.seats,data.round_id]);
  if(director&&data.mode==="blind"&&heldKey!==nextHeldKey){
    heldKey=nextHeldKey;
    const answers=data.pending_answers||[];
    $("held-title").textContent=data.round_id?"已封存答案 · "+answers.length+" / "+data.seats.length+" · 仅导演可见":"暂无待揭晓答案";
    $("held-answers").replaceChildren();
    Object.entries(data.absent||{}).forEach(([id,reason])=>$("held-answers").append(el("p","history-warning",(cast.find(s=>s.id===id)?.label||id)+" · 本轮未作答："+reason)));
    answers.forEach(answer=>{const seat=cast.find(s=>s.id===answer.speaker);const row=el("article","held-answer");row.append(el("strong","",seat?.label||answer.speaker),el("p","",answer.text));$("held-answers").append(row);});
  }
  if(isTask){
    const nextTaskKey=JSON.stringify(data.tasks);
    if(taskKey!==nextTaskKey){
    taskKey=nextTaskKey;$("task-list").replaceChildren();
    (data.tasks||[]).forEach(run=>{const seat={id:run.provider,label:run.provider,model:run.model,provider:run.provider};const card=el("article","message");const bubble=el("div","bubble");const head=el("div","message-head");head.append(el("strong","",run.provider),el("span","tag",taskStates[run.status]||run.status));bubble.append(head,el("p","task-meta",run.model+" · "+Math.round(run.elapsed_ms/1000)+" 秒"),el("p","task-meta",run.cwd),el("p","speech",run.output||"正在等待回复…"));if(run.error)bubble.append(el("p","history-warning",run.error));card.append(avatar(seat),bubble);$("task-list").append(card);});
    }
    $("status").textContent=data.archived?"会话已结束 · 历史保留":"输出由 CLI 自动提交；回复不等于验收通过";
    $("count").textContent=(data.tasks||[]).length+" 次任务";return;
  }
  const nextSeatKey=JSON.stringify([data.seats,data.speaker,data.status,selected]);
  if(seatKey!==nextSeatKey){
    $("seats").replaceChildren();
    cast.forEach(seat=>{const speaking=data.speaker===seat.id&&["running","ready"].includes(data.status);const row=el("button","seat"+(selected===seat.id?" selected":"")+(speaking?" speaking":""));row.type="button";row.setAttribute("aria-pressed",String(selected===seat.id));row.setAttribute("aria-label","只看"+seat.label+"的公开发言");row.onclick=()=>choose(selected===seat.id?null:seat.id);const detail=el("span","seat-detail");detail.append(el("strong","seat-name",seat.label),el("span","seat-model",seat.model));row.append(avatar(seat),detail);$("seats").append(row);});
    $("all-seats").setAttribute("aria-pressed",String(selected===null));seatKey=nextSeatKey;
  }
  const speaker=cast.find(s=>s.id===data.speaker);
  $("status").textContent=data.archived?"会话已结束 · 历史保留":director?((speaker&&["running","ready"].includes(data.status)?speaker.label+" · ":"")+((data.mode==="blind"&&data.status==="ready"?"会话信息不完整，等待导演处理":labels[data.status])||data.status)):"公开发言 · 全员同步";
  $("count").textContent=data.messages.length+" 条公开发言";
  $("filter-note").hidden=selected===null;$("filter-note").textContent=selected?"正在查看 "+(cast.find(s=>s.id===selected)?.label||selected)+" 的发言 · 点击「所有人的发言」恢复全场":"";
  const nextMessageKey=JSON.stringify([data.messages,selected,data.seats]);
  if(messageKey!==nextMessageKey){
    $("messages").replaceChildren();
    data.messages.forEach((message,index)=>{
      if(selected&&message.speaker!==selected)return;
      const seat=cast.find(s=>s.id===message.speaker)||{id:message.speaker,label:message.speaker,model:"",provider:""};
      const article=el("article","message"+(seat.id==="director"?" host-message":""));
      const bubble=el("div","bubble"),head=el("div","message-head");
      head.append(el("strong","",seat.label));if(seat.id==="director")head.append(el("span","tag","主持人 · 导演扮演"));
      const stamp=el("time","timestamp");stamp.dateTime=message.time;stamp.textContent=new Date(message.time).toLocaleTimeString("zh-CN",{hour:"2-digit",minute:"2-digit"});head.append(stamp);
      const meta=el("div","message-meta");meta.append(el("span","number","#"+String(index+1).padStart(3,"0")));
      const copy=el("button","copy","复制台词");copy.type="button";copy.setAttribute("aria-label","复制"+seat.label+"的第"+(index+1)+"条台词");copy.onclick=async()=>{try{await navigator.clipboard.writeText(message.text);copy.textContent="已复制";}catch{copy.textContent="请选中文字复制";}};meta.append(copy);
      bubble.append(head,el("p","speech",message.text),meta);article.append(avatar(seat),bubble);$("messages").append(article);
    });
    $("empty").hidden=$("messages").childElementCount>0;$("empty-text").textContent=selected?"这个角色还没有公开发言。":"主持人开场，选手回应。公开台词会依次留在这里。";messageKey=nextMessageKey;
  }
  if(hadData&&!privateView&&data.messages.length>previousCount){if(nearBottom&&selected===null)window.scrollTo({top:document.documentElement.scrollHeight,behavior:"instant"});else $("latest").hidden=false;}
  previousCount=data.messages.length;
}
const modeDescriptions={all:"全部用途",discussion:"轮流发言，彼此回应",blind:"先独立回答，再统一揭晓",task:"各自执行，结果仅主控可见"};
function renderHome(data){
  const shows=data.shows||[];
  const state=$("archive-filter").value, query=$("search").value.trim().toLocaleLowerCase();
  const nextKey=JSON.stringify([shows,data.unavailable,modeFilter,state,query]);
  if(homeKey===nextKey)return;
  homeKey=nextKey;
  const scope=shows.filter(show=>state==="all"||(state==="ended"?show.archived:!show.archived));
  for(const button of $("mode-filters").children){
    const mode=button.dataset.mode;
    button.setAttribute("aria-pressed",String(modeFilter===mode));
    button.querySelector(".mode-count").textContent=scope.filter(show=>mode==="all"||show.mode===mode).length;
  }
  const matching=scope.filter(show=>(modeFilter==="all"||show.mode===modeFilter)&&show.title.toLocaleLowerCase().includes(query));
  $("list-count").textContent=matching.length+" 个会话";
  $("conversation-list").replaceChildren();
  matching.forEach(show=>{
    const card=el("article","conversation-card"), info=el("div","conversation-info"), title=el("h2");
    const link=el("a","conversation-title",show.title);link.href=show.director_url;title.append(link);
    info.append(el("span","tag mode-"+show.mode,modes[show.mode]||show.mode),title,el("p","",show.archived?"已结束 · 历史保留":modeDescriptions[show.mode]));
    const open=el("a","open-conversation",show.mode==="task"?"查看任务 →":"进入主控 →");open.href=show.director_url;open.setAttribute("aria-label",(show.mode==="task"?"查看任务：":"进入主控：")+show.title);
    card.append(info,open);$("conversation-list").append(card);
  });
  if(!matching.length)$("conversation-list").append(el("p","list-empty",shows.length?"没有匹配的会话。试试其他用途、状态或名称。":"还没有会话。展开「如何开始新会话？」查看示例。"));
  (data.unavailable||[]).forEach(item=>$("conversation-list").append(el("p","history-warning","有一份历史暂不可用，可能被另一服务占用或无法读取。记录："+item.record)));
}
for(const mode of ["all",...Object.keys(modes)]){
  const button=el("button","mode-filter");button.type="button";button.dataset.mode=mode;
  const title=el("span","mode-title",mode==="all"?"全部":modes[mode]);title.append(el("span","mode-count","0"));
  button.append(title,el("span","mode-description",modeDescriptions[mode]));
  button.onclick=()=>{modeFilter=mode;if(current)renderHome(current);};$("mode-filters").append(button);
}
const examples=["开一个一起聊的节目，让已连接的 CLI 讨论：……","开一个各自答的节目，让各 CLI 独立回答：……，收齐后统一揭晓。","让 CLI 完成任务：……，将结果放进同一个任务会话。"];
examples.forEach((text,index)=>{
  const row=el("div","start-example"), copy=el("button","copy","复制示例");
  copy.type="button";copy.setAttribute("aria-label","复制"+Object.values(modes)[index]+"示例");
  copy.onclick=async()=>{try{await navigator.clipboard.writeText(text);copy.textContent="已复制，粘贴到主控对话";}catch{copy.textContent="请选中文字复制";}};
  row.append(el("p","",text),copy);$("start-examples").append(row);
});
$("search").oninput=()=>{if(current)renderHome(current);};
$("archive-filter").onchange=()=>{if(current)renderHome(current);};
$("conversation-tab").onclick=()=>{privateView=false;if(current)render(current);};
$("backstage-tab").onclick=()=>{privateView=true;if(current)render(current);};
$("all-seats").onclick=()=>choose(null);
$("latest").onclick=()=>{choose(null);window.scrollTo({top:document.documentElement.scrollHeight,behavior:"smooth"});$("latest").hidden=true;};
async function refresh(){try{const response=await fetch("./state",{cache:"no-store"});if(!response.ok)throw Error("offline");render(await response.json());$("connection").textContent="本地已连接";}catch{$("connection").textContent="连接断开 · 保留上次画面";}setTimeout(refresh,1500);}
refresh();
