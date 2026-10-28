import fs from 'node:fs';
import {dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
const directory=dirname(fileURLToPath(import.meta.url));
const raw=JSON.parse(fs.readFileSync(directory+'/evidence.json','utf8'));
const W=1500,L=235,R=75,P=W-L-R;
const C={ink:'#17212d',muted:'#596878',grid:'#e4e9ee',http:'#8db2df',server:'#eaa557',apply:'#68b59c',watch:'#94b958',visible:'#b44546'};
const esc=s=>String(s).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;');
const text=(x,y,s,size=16,c=C.ink,anchor='start')=>`<text x="${x}" y="${y}" font-size="${size}" fill="${c}" text-anchor="${anchor}">${esc(s)}</text>`;
const line=(x1,y1,x2,y2,c=C.grid)=>`<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" stroke="${c}" stroke-width="1"/>`;
function transactionTracks(records) {
 const tracks=[];
 for(const e of [...records].sort((a,b)=>a.startMs-b.startMs)) {
  let t=tracks.find(t=>t.end<=e.startMs);
  if(!t){t={end:0,events:[]};tracks.push(t);}
  t.events.push(e);t.end=e.endMs;
 }
 return tracks;
}
const trackCount=Math.max(...['off','on'].map(k=>transactionTracks(raw[k].selected.transactions).length));
const rows=trackCount+4, rowHeight=28,panelHeight=rows*rowHeight+105;
function panel(top,max,step,label,sample,zoom=false){
 const scale=P/max,x=t=>L+t*scale,y0=top+68;
 let svg=text(32,top+23,label,20);
 for(let t=0;t<=max;t+=step)svg+=line(x(t),top+50,x(t),y0+(rows-1)*rowHeight+12)+text(x(t),top+44,t,13,C.muted,'middle');
 svg+=text(W-18,top+44,'ms',13,C.muted,'end');
 const rect=(e,row,c)=>{
  if(e.startMs>=max||e.endMs<=0)return '';
  return `<rect x="${x(Math.max(0,e.startMs))}" y="${y0+row*rowHeight-8}" width="${Math.max(.8,(Math.min(max,e.endMs)-Math.max(0,e.startMs))*scale)}" height="16" rx="2" fill="${c}"/>`;
 };
 const labels=['客户端 HTTP Pull',...Array.from({length:trackCount},(_,i)=>`后端事务 ${i+1}`),'SQLite apply','Rust watch 更新','应用结果回调'];
 labels.forEach((s,i)=>{svg+=text(L-14,y0+i*rowHeight+5,s,15,C.ink,'end');});
 sample.pulls.forEach(e=>{svg+=rect(e,0,C.http);});
 transactionTracks(sample.transactions).forEach((t,i)=>t.events.forEach(e=>{svg+=rect(e,1+i,C.server);}));
 sample.apply.forEach(e=>{svg+=rect(e,1+trackCount,C.apply);});
 sample.watch.forEach(e=>{svg+=rect(e,2+trackCount,C.watch);});
 for(const e of sample.snapshots.filter(e=>e.rows>0)){
  const at=e.atUs/1000;if(at>max)continue;
  svg+=`<circle cx="${x(at)}" cy="${y0+(3+trackCount)*rowHeight}" r="3.5" fill="${C.visible}"/>`;
  if((zoom&&e.rows<=1000)||(!zoom&&[500,5000,10000].includes(e.rows)))svg+=text(x(at),y0+(3+trackCount)*rowHeight+24,`${e.rows.toLocaleString()} 条`,13,C.visible,'middle');
 }
 return svg;
}
async function main(){
 const off=raw.off.selected,on=raw.on.selected;
 const metric=raw.comparison.find(r=>r.case==='initial-sync/10000/0'&&r.metric==='allVisibleMs');
 const max=Math.ceil(Math.max(off.metrics.allVisibleMs[0],on.metrics.allVisibleMs[0])/200)*200;
 const H=185+panelHeight*4+140;
 let svg=`<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}"><style>text{font-family:'Arial Unicode MS','Heiti SC',sans-serif}</style><rect width="${W}" height="${H}" fill="#fff"/>`;
 svg+=text(32,42,'首次同步 10k：单页预取对照',28);
 svg+=text(32,75,`5 次中位数：${metric.before.toFixed(1)} → ${metric.after.toFixed(1)} ms（${metric.changePercent.toFixed(1)}%）· 每页 500 条 · 每条文本 512 字节`,17,C.muted);
 svg+=text(32,104,'同一 release native + Dart AOT 构建；每幅图取该组“全部可见”中位数对应的完整样本。',15,C.muted);
 let top=130;
 svg+=panel(top,max,200,`预取关闭 · 全部可见 ${off.metrics.allVisibleMs[0].toFixed(3)} ms · 第 ${raw.off.selected.repetition} 次`,off);top+=panelHeight;
 svg+=panel(top,max,200,`预取开启 · 全部可见 ${on.metrics.allVisibleMs[0].toFixed(3)} ms · 第 ${raw.on.selected.repetition} 次`,on);top+=panelHeight;
 svg+=line(32,top-10,W-32,top-10);
 svg+=panel(top,300,25,'前 300 ms 放大 · 预取关闭',off,true);top+=panelHeight;
 svg+=panel(top,300,25,'前 300 ms 放大 · 预取开启',on,true);top+=panelHeight;
 svg+=text(32,top+18,'所有泳道按真实起止时间对齐；并发事务使用不同泳道，累计 span 时长不能相加。',15,C.muted);
 svg+=text(32,top+46,`首次同步对照共 10 个样本已通过；最大位置不确定度 ±${Math.max(raw.off.maxPlacementUncertaintyUs,raw.on.maxPlacementUncertaintyUs).toFixed(1)} µs。`,15,C.muted);
 svg+=text(32,top+74,'这是本机 elapsed-time trace；空白区间尚未归因。不是 CPU profile，也不是与 PowerSync 的同条件对照。',15,C.muted);
 svg+='</svg>';
 fs.writeFileSync(directory+'/timeline.svg',svg);
 console.log(directory+'/timeline.svg');
}
main().catch(e=>{console.error(e);process.exitCode=1;});
