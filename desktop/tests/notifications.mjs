// Real typed IPC and WinRT. Local ACP fixture only; no model or user profile.
import {chromium} from '../../frontend/node_modules/playwright/index.mjs';
import {spawn,execFileSync} from 'node:child_process';
import {mkdtemp,mkdir,copyFile,readFile,writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {randomUUID} from 'node:crypto';
import {createServer} from 'node:net';
import assert from 'node:assert/strict';
const root=await mkdtemp(join(tmpdir(),'branchline-notifications-'));
const report={date:new Date().toISOString(),root,checks:[],status:'FAIL'};
const wait=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,ms=45000){const end=Date.now()+ms;while(Date.now()<end){if(await fn())return;await wait(100);}throw Error('deadline: '+report.stage);}
const stage=s=>{report.stage=s;console.log(s);};
const repo=join(root,'repo тест'),tree=join(root,'linked Ю space');await mkdir(repo);
const git=(...args)=>execFileSync('git.exe',['-C',repo,...args],{windowsHide:true,stdio:'pipe'});
git('init','-b','main');git('-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','--allow-empty','-m','fixture');git('worktree','add','-b','linked',tree);
await mkdir(join(root,'tools'));const harness=join(root,'tools','opencode.exe');await copyFile(process.env.NOTIFICATIONS_FIXTURE_EXE??'desktop/target/release/examples/acp-fixture.exe',harness);
const socket=createServer();await new Promise(r=>socket.listen(0,'127.0.0.1',r));const port=socket.address().port;await new Promise(r=>socket.close(r));
const env={...process.env,AWM_CONFIG:join(root,'app.json'),ACP_FIXTURE_ROOT:root,WEBVIEW2_USER_DATA_FOLDER:join(root,'webview'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`};
let app,browser,page;
try{
 stage('launch');app=spawn(process.env.NOTIFICATIONS_APP_EXE??resolve('desktop/target/release/agent-workspace-desktop.exe'),[],{cwd:root,env,windowsHide:true,stdio:'ignore'});
 await until(async()=>{if(app.exitCode!==null)throw Error('app exited');try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`,{timeout:1000});return true;}catch{return false;}});
 await until(()=>{page=browser.contexts()[0].pages().find(p=>/tauri/.test(p.url()));return !!page;});
 const invoke=async(name,args)=>{const result=await page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args).catch(error=>({__error:error})),{name,args});if(result?.__error)throw Object.assign(new Error(result.__error.message),result.__error);return result;};
 await page.evaluate(async()=>{
  window.nativeNotificationActivations=[];
  const handler=window.__TAURI_INTERNALS__.transformCallback(event=>window.nativeNotificationActivations.push(event.payload));
  await window.__TAURI_INTERNALS__.invoke('plugin:event|listen',{event:'attention-open',target:{kind:'Any'},handler});
 });
 const environment=await invoke('runtime_environment'),scope={environment_id:environment.id};
 const project=await invoke('add_project',{scope,rootPath:repo});
 const worktree=(await invoke('list_worktrees',{scope:{...scope,project_id:project.id}})).worktrees.find(t=>!t.is_main);
 const ctx={project_id:project.id,worktree_id:worktree.id};
 const agents=action=>invoke('agents',{scope,request:{...ctx,request_id:randomUUID(),...action}});
 const added=await invoke('harnesses',{scope,request:{action:'add',request_id:randomUUID(),executable:harness}});
 const instance_id=randomUUID();await agents({action:'launch',installation_id:added.installations.find(i=>i.executable===harness).id,instance_id});
 await until(async()=>(await agents({action:'list'})).instances.some(i=>i.id===instance_id&&i.connection==='connected'));
 await agents({action:'new',instance_id});let instance;
 await until(async()=>{instance=(await agents({action:'list'})).instances.find(i=>i.id===instance_id);return instance.sessions.length>0;});
 const session_id=instance.sessions[0].id;
 await agents({action:'prompt',instance_id,session_id,text:'LOCAL_FIXTURE_PRIVATE_SENTINEL'});
 await until(async()=>{instance=(await agents({action:'list'})).instances.find(i=>i.id===instance_id);return instance.events.some(e=>e.kind==='attention_waiting');});
 const target={...scope,...ctx,instance_id,session_id,sequence:instance.events.find(e=>e.kind==='attention_waiting').sequence};
 await assert.rejects(()=>invoke('notify_attention',{target}));report.checks.push('opt-out rejected');
 const saved=await invoke('load_workspace');await invoke('save_workspace_ui',{selection:saved.selection,layout:{...saved.layout,preferences:{...saved.layout.preferences,notifications:true,system_notifications:true}}});
 await assert.rejects(()=>invoke('notify_attention',{target:{...target,instance_id:randomUUID()}}));report.checks.push('forged target rejected');
 if(process.argv.includes('--fallback')){
  stage('in-app fallback');
  await invoke('save_workspace_ui',{selection:saved.selection,layout:{...saved.layout,preferences:{...saved.layout.preferences,notifications:true,system_notifications:false}}});
  await page.getByRole('button',{name:'Настройки',exact:true}).click();await page.getByLabel('Attention по подтверждённым событиям').check();await page.getByLabel('Windows notifications',{exact:false}).uncheck();await page.getByLabel('Закрыть окно').click();
  await wait(2400);await agents({action:'new',instance_id});let second;
  await until(async()=>{instance=(await agents({action:'list'})).instances.find(i=>i.id===instance_id);second=instance.sessions.find(s=>s.id!==session_id);return !!second;});
  await page.locator('.worktree-select').filter({hasText:'repo тест'}).first().click();
  await wait(2400); // Seed the scoped ledger after the registered tree is visible.
  await agents({action:'prompt',instance_id,session_id:second.id,text:'LOCAL_FIXTURE_PRIVATE_SENTINEL_2'});
  await until(async()=>Number(await page.getByRole('button',{name:'Attention',exact:true}).locator('.notification-count').textContent().catch(()=>0))>0);
  const clickItem=async()=>{await page.getByRole('button',{name:'Attention',exact:true}).click();await page.getByRole('group',{name:'Attention',exact:true}).getByRole('button').filter({hasText:second.id}).first().click();};
  await clickItem();await until(async()=>(await invoke('load_workspace')).selection?.worktree_id===worktree.id);
  await until(async()=>(await page.getByLabel('Agent session').inputValue().catch(()=>''))===`${instance_id}/${second.id}`);
  report.checks.push('real in-app Attention from another worktree opens exact environment/repository/worktree/instance/session');
  await agents({action:'stop',instance_id});
  await until(async()=>!(await agents({action:'list'})).instances.some(i=>i.id===instance_id&&i.connection==='connected'));
  await clickItem();await until(async()=>(await page.locator('.daily-message').allTextContents()).join(' ').includes('недоступна')||(await page.locator('.daily-message').allTextContents()).join(' ').includes('не доступна'));
  report.checks.push('closed instance target reports unavailable without relaunch');
  await invoke('remove_project',{scope:{...scope,project_id:project.id}});
  await until(async()=>await page.locator('.worktree-select').filter({hasText:'linked Ю space'}).count()===0);
  const before=(await invoke('load_workspace')).selection;await clickItem();await wait(200);assert.deepEqual((await invoke('load_workspace')).selection,before);
  report.checks.push('removed registration/late retained target does not switch context');report.status='PASS_FALLBACK_ONLY';
 }else{
 await page.getByRole('button',{name:'Настройки',exact:true}).click();await page.getByLabel('Attention по подтверждённым событиям').check();await page.getByLabel('Windows notifications',{exact:false}).check();await page.getByLabel('Закрыть окно').click();
 await page.locator('.worktree-select').filter({hasText:'repo тест'}).first().click();
 await until(async()=>(await invoke('load_workspace')).layout.preferences.system_notifications);
 if(process.env.NOTIFICATIONS_WAIT_SHOW==='1'){
  await writeFile('.local-validation/notification-ready.json',JSON.stringify({...report,port,pid:app.pid,target},null,2));stage('ready before show');
  await until(async()=>{try{return (await readFile('.local-validation/notification-control.txt','utf8')).trim()==='show';}catch{return false;}},600000);
 }
 stage('native show');try{report.availabilityDiagnostic=await invoke('notify_attention',{target});report.nativeShow='accepted';}catch(error){report.nativeShow={code:error.code,message:error.message};console.log('NATIVE '+JSON.stringify(report.nativeShow));}
 report.port=port;report.pid=app.pid;report.target=target;
 await writeFile('.local-validation/notification-ready.json',JSON.stringify(report,null,2));
 if(report.nativeShow==='accepted'){
  await invoke('notify_attention',{target});report.checks.push('duplicate IPC accepted without reshow');
  stage('await real OS activation');
  await until(async()=>{if(await page.evaluate(()=>window.nativeNotificationActivations.length>0))return true;try{return (await readFile('.local-validation/notification-control.txt','utf8')).trim()==='close';}catch{return false;}},600000);
  report.nativeActivations=await page.evaluate(()=>window.nativeNotificationActivations);
  if(report.nativeActivations.length){
   assert.deepEqual(report.nativeActivations[0],target);
   await until(async()=>(await page.getByLabel('Agent session').inputValue().catch(()=>''))===`${instance_id}/${session_id}`);
   assert.deepEqual((await invoke('load_workspace')).selection,{...scope,...ctx});
   report.checks.push('actual OS toast activation opens exact environment/repository/worktree/instance/session from another selected context');
   await page.screenshot({path:'docs/desktop-notification-click.png'});
  }
  report.selection=(await invoke('load_workspace')).selection;
  report.session=await page.getByLabel('Agent session').inputValue().catch(()=>null);
 }
 await agents({action:'stop',instance_id});await assert.rejects(()=>invoke('notify_attention',{target}));report.checks.push('stopped/disconnected target rejected');
 report.status=report.nativeActivations?.length?'PASS_NATIVE_CLICK':report.nativeShow==='accepted'?'AWAITING_VISUAL_ACCEPTANCE':'PARTIAL';
 }
}catch(error){report.error=String(error.stack??error);throw error;}
finally{
 if(app?.exitCode===null&&page){await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('close_application')).catch(()=>{});await until(()=>app.exitCode!==null,20000).catch(()=>{});}
 await browser?.close().catch(()=>{});await writeFile(`docs/desktop-notifications${process.argv.includes('--fallback')?'-fallback':''}-results.json`,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));
}
