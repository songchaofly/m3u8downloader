import assert from 'node:assert/strict';
import {connectPage} from './native-cdp.mjs';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {mkdir,writeFile,readFile,open as openFile,mkdtemp,rm} from 'node:fs/promises';
import path from 'node:path';

const root=process.cwd(),resultDir=path.join(root,'test-results');await mkdir(resultDir,{recursive:true});
const exe=process.env.M3U8_TEST_EXE||path.join(root,'target/release/m3u8-studio.exe');
const server=spawn('python',['-X','utf8','scripts/performance_server.py'],{windowsHide:true});
const fixture=await new Promise(resolve=>createInterface({input:server.stdout}).on('line',line=>{if(line.startsWith('{'))resolve(JSON.parse(line));}));
const report={fixture,automation:'Minimal CDP Runtime.evaluate; no Playwright injection; loopback debug endpoint enabled only by test',startup_ms:[],baseline_ms:[],application_ms:[],feedback_ms:[],memory_reports:[]};
let app,browser;
async function open(index){
  const port=19500+index,start=performance.now();
  app=spawn(exe,[],{windowsHide:true,env:{...process.env,M3U8_STUDIO_DATA_DIR:path.join(root,'.local','performance',`中文 空格-${Date.now()}-${index}`),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`}});
  for(let n=0;n<120;n++){try{browser=await connectPage(port);break;}catch{await new Promise(r=>setTimeout(r,50));}}
  if(!browser)throw new Error('Native startup failed');
  const page=browser;let ready=false;
  for(let n=0;n<300;n++){ready=await page.evaluate(()=>document.querySelector('h1')?.textContent==='下载工作台');if(ready)break;await new Promise(r=>setTimeout(r,50));}
  assert.ok(ready,'Workspace did not render');report.startup_ms.push(performance.now()-start);
  // Accept the local usage notice through the same controls as a normal user.
  await page.evaluate(async()=>{
    const label=[...document.querySelectorAll('label')].find(el=>el.textContent.includes('我已了解并同意合理使用本工具'));
    if(label){label.querySelector('input').click();await new Promise(requestAnimationFrame);[...document.querySelectorAll('button')].find(el=>el.textContent.trim()==='进入工作台').click();}
  });
  return page;
}
async function close(){if(browser)await browser.close();browser=null;if(app){if(app.exitCode===null){const exited=new Promise(resolve=>app.once('exit',resolve));app.kill();await exited;}app=null;}}
async function baseline(){
  const parent=path.join(root,'.local','performance');await mkdir(parent,{recursive:true});
  const folder=await mkdtemp(path.join(parent,'baseline-'));
  let next=0;const start=performance.now();
  await Promise.all(Array.from({length:8},async()=>{
    while(next<fixture.segments){
      const index=next++;const response=await fetch(`${fixture.url}/segment/${index}.ts`);const body=await response.arrayBuffer();
      if(body.byteLength!==fixture.segment_bytes)throw new Error('Baseline length mismatch');
      const file=await openFile(path.join(folder,`${index}.ts`),'wx');
      try{await file.writeFile(Buffer.from(body));await file.sync();}finally{await file.close();}
    }
  }));
  const elapsed=performance.now()-start;
  if(!path.resolve(folder).startsWith(path.resolve(parent)+path.sep))throw new Error('Unexpected baseline cleanup path');
  await rm(folder,{recursive:true});return elapsed;
}
try {
  for(let index=0;index<Number(process.env.M3U8_PERF_STARTS||5);index++){await open(index);await close();}
  const page=await open(5);report.startup_ms.pop();
  const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const settings=await invoke('get_settings');settings.notify=false;settings.low_disk_mib=0;await invoke('save_settings',{settings});
  for(let run=0;run<Number(process.env.M3U8_PERF_RUNS||3);run++){
    report.baseline_ms.push(await baseline());
    const memoryFile=path.join(resultDir,`performance-memory-${Date.now()}-${run}.json`);
    const sampler=spawn('python',['scripts/sample-memory.py',String(app.pid),memoryFile],{windowsHide:true});
    const start=performance.now();
    const task=await invoke('add_task',{input:{source:fixture.url+'/index.m3u8',title:`Performance ${run}`,output_dir:settings.output_dir,keep_cache:true,ffmpeg:'intentionally-unavailable-media-tool.exe'},start:true});
    const feedback=await page.evaluate(async()=>{
      const times=[];const input=document.querySelector('input[placeholder="搜索任务…"]');
      for(let i=0;i<100;i++){const start=performance.now();input.value=`unmatched-${i}`;input.dispatchEvent(new Event('input',{bubbles:true}));await new Promise(requestAnimationFrame);times.push(performance.now()-start);}
      input.value='';input.dispatchEvent(new Event('input',{bubbles:true}));return times;
    });report.feedback_ms.push(...feedback);
    let view;
    for(let poll=0;poll<12000;poll++){view=(await invoke('list_tasks')).find(t=>t.id===task.id);if(view.state==='failed')break;await new Promise(r=>setTimeout(r,25));}
    report.application_ms.push(performance.now()-start);
    assert.equal(view.done,1000);assert.equal(view.bytes,fixture.segment_bytes*1000);assert.equal(view.error.code,'FFMPEG_UNAVAILABLE');
    await writeFile(memoryFile+'.stop','stop');await new Promise(resolve=>sampler.once('exit',resolve));
    const memory=JSON.parse(await readFile(memoryFile,'utf8'));assert.ok(memory.samples.length>10);report.memory_reports.push(memory);
    await invoke('task_action',{id:task.id,action:'delete_cache'});
  }
  const median=values=>[...values].sort((a,b)=>a-b)[Math.floor(values.length/2)];
  report.throughput_ratio=median(report.baseline_ms)/median(report.application_ms);
  report.feedback_p95_ms=[...report.feedback_ms].sort((a,b)=>a-b)[Math.ceil(report.feedback_ms.length*.95)-1];
  report.peak_gui_core_bytes=Math.max(...report.memory_reports.map(r=>r.peak_gui_core_bytes));
  report.passed={startup:report.startup_ms.every(n=>n<3000),throughput:report.throughput_ratio>=.8,feedback:report.feedback_p95_ms<200,memory:report.peak_gui_core_bytes<512*1024*1024};
  await writeFile(path.join(resultDir,'performance.json'),JSON.stringify(report,null,2));
  console.log(JSON.stringify({...report,memory_reports:report.memory_reports.map(({samples,...metadata})=>metadata),feedback_ms:'raw samples in performance.json'},null,2));
  if(Object.values(report.passed).some(value=>!value))process.exitCode=1;
} finally {await close();server.kill();}
