// ═══════════════════════════════════════════════════════════════════
//  STEEL CAPTURE — Browser Visualization
//  Architecture: Source → SensorPacket → Coordinator → CaptureFrame → Render
//  Sources: built-in sim, JSON file playback, WebSocket stream
// ═══════════════════════════════════════════════════════════════════

var SC=['#e74c3c','#e67e22','#b8960f','#2ecc71','#1abc9c','#3498db','#2980b9','#9b59b6','#8e44ad','#e91e63'];
var NN=['C','C#','D','D#','E','F','F#','G','G#','A','A#','B'];
var ML=43,MH=86,RS=8,HM=800,AMP_FLOOR=0.01;

// ═══ ABSTRACT COPEDANT DATA MODEL ═══
// Serializable config — can be loaded from JSON, swapped at runtime
var CFG={
  copedant:{
    name:'Geoff Derby E9',type:'S-10',config:'3+5',strings:10,
    open_notes:['F#4','D#4','G#4','E4','B3','G#3','F#3','E3','D3','B2'],
    open_midi:[66,63,68,64,59,56,54,52,50,47],
    pedal_names:['P1','P2','P3'],
    lever_names:['LKL1','LKV','LKR','RKL','RKR'],
    pedal_changes:{P1:[[4,2],[9,2]],P2:[[2,1],[5,1]],P3:[[3,2],[4,2]]},
    lever_changes:{LKL1:[[3,1],[7,1]],LKV:[[4,-1],[9,-1]],LKR:[[3,-1],[7,-1]],RKL:[[0,2],[1,1],[6,2]],RKR:[[1,-1],[5,-2],[8,-1]]}
  },
  instrument:{
    sensor_positions_fret:[0,5,10,15],
    scale_length_inches:24.5
  }
};

// Derived runtime arrays (rebuilt when copedant changes)
var OM,PC,LC,SFP,SCALE_LEN;
// pedal/lever string lists for attack detection
var PS,LS;

function applyCopedant(cfg){
  var c=cfg.copedant,inst=cfg.instrument;
  OM=c.open_midi.slice();
  SFP=inst.sensor_positions_fret.slice();
  SCALE_LEN=inst.scale_length_inches;
  // Build PC (pedal changes) as array-of-arrays matching pedal index
  PC=c.pedal_names.map(function(pn){return c.pedal_changes[pn]||[]});
  // Build LC (lever changes) as array-of-arrays matching lever index
  LC=c.lever_names.map(function(ln){return c.lever_changes[ln]||[]});
  // Build PS/LS: which strings each pedal/lever affects (for attack detection)
  PS=PC.map(function(changes){return changes.map(function(e){return e[0]})});
  LS=LC.map(function(changes){return changes.map(function(e){return e[0]})});
  if(typeof coordReset==='function')coordReset();
  if(typeof resetFlash==='function')resetFlash();
  if(typeof updateNames==='function')updateNames();
}
applyCopedant(CFG);

// Serialize current config to JSON (for export/save)
function exportConfig(){return JSON.stringify(CFG,null,2)}
// Load config from JSON (for import)
function importConfig(json){
  var c=typeof json==='string'?JSON.parse(json):json;
  CFG=c;applyCopedant(CFG)}

// Pedal/lever display names (derived from copedant)
var PNM,LNM;
// Standard E9 tab notation: P1=A, P2=B, P3=C
var TAB_PED=['A','B','C','D','E','F','G','H'];
var TAB_LEV=['LKL','LKV','LKR','RKL','RKR','RL6','RL7','RL8'];
function updateNames(){PNM=CFG.copedant.pedal_names;LNM=CFG.copedant.lever_names}
updateNames();

// ═══ FRET GEOMETRY ═══
function fretToPhysical(f){if(f===null)return null;return 1-Math.pow(2,-f/12)}
function physicalToFret(p){if(p===null||p<=0)return 0;if(p>=1)return 24;return -12*Math.log2(1-p)}
function parallax(p){return p*(.88+.12*p)}

// ═══ COPEDANT ═══
function cpd(fret,ped,lev){var r=[];
  var nStr=OM.length;
  for(var i=0;i<nStr;i++){var m=OM[i];
    for(var j=0;j<PC.length;j++)for(var k=0;k<PC[j].length;k++){var e=PC[j][k];if(e[0]===i)m+=e[1]*(ped[j]||0)}
    for(var j2=0;j2<LC.length;j2++)for(var k2=0;k2<LC[j2].length;k2++){var e2=LC[j2][k2];if(e2[0]===i)m+=e2[1]*(lev[j2]||0)}
    if(fret!==null)m+=fret;r.push(440*Math.pow(2,(m-69)/12))}return r}
function h2m(hz){return 69+12*Math.log2(hz/440)}
function m2n(m){var ri=Math.round(m);return NN[((ri%12)+12)%12]+(Math.floor(ri/12)-1)}
function sm(t){t=Math.max(0,Math.min(1,t));return t*t*(3-2*t)}
function sensorAt(barFret,sensFret){
  if(barFret===null)return 0;
  var bp=fretToPhysical(barFret)*SCALE_LEN,sp=fretToPhysical(sensFret)*SCALE_LEN;
  var d=Math.abs(bp-sp)/2.0;return Math.min(1,1/Math.pow(1+d*d,1.5))}
function sensResp(bar){return SFP.map(function(sf){return sensorAt(bar,sf)})}

// ═══ COORDINATOR ═══
var coord={prevActive:new Array(10).fill(false),prevPedalEng:new Array(3).fill(false),
  prevLeverEng:new Array(5).fill(false),amp:new Array(10).fill(0)};

function fuseBarPosition(sens){
  var totalE=0;for(var i=0;i<SFP.length;i++)totalE+=sens[i];
  if(totalE<.08)return{pos:null,conf:0,src:'None'};
  var bestF=0,bestErr=1e9;
  for(var f=0;f<=16;f+=.5){var err=0;
    for(var i=0;i<SFP.length;i++){var expected=sensorAt(f,SFP[i]);var d=expected-sens[i];err+=d*d}
    if(err<bestErr){bestErr=err;bestF=f}}
  var lo=Math.max(0,bestF-.5),hi=Math.min(16,bestF+.5);
  for(var iter=0;iter<12;iter++){
    var m1=lo+(hi-lo)*.382,m2=lo+(hi-lo)*.618,e1=0,e2=0;
    for(var i=0;i<SFP.length;i++){var d1=sensorAt(m1,SFP[i])-sens[i];e1+=d1*d1;
      var d2=sensorAt(m2,SFP[i])-sens[i];e2+=d2*d2}
    if(e1<e2)hi=m2;else lo=m1}
  var pos=(lo+hi)/2,conf=Math.min(1,totalE*.35);
  return{pos:pos,conf:conf,src:conf>.25?'Fused':'Sensor'}}

function coordProcess(pkt,dt){
  var nStr=OM.length,nPed=PC.length,nLev=LC.length;
  var fused=fuseBarPosition(pkt.bar_sens);
  var pitches=cpd(fused.pos,pkt.pedals,pkt.levers);
  var atk=new Array(nStr).fill(false);
  // Attack on new pick activation
  for(var i=0;i<nStr;i++){if(pkt.picks[i]&&!coord.prevActive[i])atk[i]=true}
  // Attack on pedal change (for active strings affected by that pedal)
  var pe=[];for(var j=0;j<nPed;j++)pe.push((pkt.pedals[j]||0)>.5);
  for(var j=0;j<nPed;j++){if(pe[j]!==coord.prevPedalEng[j])for(var k=0;k<PS[j].length;k++)if(pkt.picks[PS[j][k]])atk[PS[j][k]]=true}
  coord.prevPedalEng=pe;
  // Attack on lever change
  var le=[];for(var j2=0;j2<nLev;j2++)le.push((pkt.levers[j2]||0)>.5);
  for(var j2=0;j2<nLev;j2++){if(le[j2]!==coord.prevLeverEng[j2])for(var k2=0;k2<LS[j2].length;k2++)if(pkt.picks[LS[j2][k2]])atk[LS[j2][k2]]=true}
  coord.prevLeverEng=le;
  coord.prevActive=pkt.picks.slice();
  // Amplitude envelope
  for(var i=0;i<nStr;i++){
    if(atk[i]){coord.amp[i]=(pkt.picks[i]&&coord.amp[i]<.1)?1:Math.max(coord.amp[i],.6);atkFlash[i]=0.4}
    coord.amp[i]*=Math.exp((pkt.picks[i]?-1.2:-12)*dt);if(coord.amp[i]<AMP_FLOOR)coord.amp[i]=0}
  return{timestamp_us:pkt.timestamp_us,pedals:pkt.pedals.slice(),knee_levers:pkt.levers.slice(),
    volume:pkt.volume,bar_position:fused.pos,bar_confidence:fused.conf,bar_source:fused.src,
    bar_sensors:pkt.bar_sens.slice(),string_pitches_hz:pitches,string_active:pkt.picks.slice(),
    attacks:atk,string_amp:coord.amp.slice()}}
function coordReset(){if(typeof coord==='undefined')return;var n=OM.length;coord.prevActive=new Array(n).fill(false);
  coord.prevPedalEng=new Array(PC.length).fill(false);
  coord.prevLeverEng=new Array(LC.length).fill(false);coord.amp=new Array(n).fill(0)}
var S=null,H=[],atkFlash=[];
function resetFlash(){atkFlash=new Array(OM.length).fill(0)}
resetFlash();

// Control history for sparklines (ring buffer, ~4s at 60fps)
var CTRL_HIST_LEN=240;
var ctrlHist={pedals:[],levers:[],volume:[]};
function resetCtrlHist(){
  ctrlHist.pedals=[];ctrlHist.levers=[];ctrlHist.volume=[];
  for(var i=0;i<CTRL_HIST_LEN;i++){
    ctrlHist.pedals.push(new Array(PC.length).fill(0));
    ctrlHist.levers.push(new Array(LC.length).fill(0));
    ctrlHist.volume.push(0)}}
resetCtrlHist();
function pushCtrlHist(){
  if(!S)return;
  ctrlHist.pedals.push(S.pedals.slice());if(ctrlHist.pedals.length>CTRL_HIST_LEN)ctrlHist.pedals.shift();
  ctrlHist.levers.push(S.knee_levers.slice());if(ctrlHist.levers.length>CTRL_HIST_LEN)ctrlHist.levers.shift();
  ctrlHist.volume.push(S.volume);if(ctrlHist.volume.length>CTRL_HIST_LEN)ctrlHist.volume.shift()}
function pushFrame(f){S=f;H.push(f);if(H.length>HM)H.splice(0,H.length-HM)}

// ═══ SOURCE ABSTRACTION ═══
// Sources produce SensorPackets. Three types:
//   'sim'  — built-in procedural demo (loops)
//   'file' — JSON playback from loaded file (loops)
//   'ws'   — WebSocket stream (live)

var sources={};  // registry: name → {type, label, data?, rate?, duration?}
var curSrc='sim';
var fileCursor=0, fileStartT=0, fileLooping=true;

// Register a JSON dataset as a playback source
function registerFileSource(name, label, json){
  sources[name]={type:'file',label:label,data:json.packets,rate:json.sample_rate_hz||60,
    duration:json.duration_s||(json.packets.length/(json.sample_rate_hz||60))};
  rebuildSourceUI()}

// Get next packet from current source
function sourceNext(dt, wallTime){
  var src=sources[curSrc];
  if(!src)return null;
  if(src.type==='sim') return simGen(dt);
  if(src.type==='file'){
    if(!src.data||src.data.length===0)return null;
    var elapsed=(wallTime-fileStartT)*1000; // ms
    var pktDur=1000/src.rate;
    var idx=Math.floor(elapsed/pktDur);
    if(idx<0)return null;
    if(idx>=src.data.length){
      if(fileLooping){fileStartT=wallTime;fileCursor=0;H=[];coordReset();return sourceNext(0,wallTime)}
      return null}
    if(idx===fileCursor)return null; // no new packet yet
    fileCursor=idx;
    var p=src.data[idx];
    return{timestamp_us:p.t_us||Math.floor(idx*pktDur*1000),
      pedals:p.ped||[0,0,0],levers:p.lev||[0,0,0,0,0],
      bar_sens:p.sens||[0,0,0,0],volume:p.vol!=null?p.vol:0,
      picks:p.picks?p.picks.map(Boolean):new Array(10).fill(false)}}
  return null}

function switchSource(name){
  if(!sources[name])return;
  curSrc=name;
  H=[];coordReset();simTime=0;simPhase=-1;fileCursor=0;
  fileStartT=performance.now()/1000;
  if(typeof actx!=='undefined'&&actx&&soundOn)audioStartT=actx.currentTime;
  rebuildSourceUI()}

// ═══ SOURCE MANAGEMENT ═══

function rebuildSourceUI(){
  var el=document.getElementById('srcBtns');if(!el)return;
  var html='';
  var names=Object.keys(sources);
  for(var i=0;i<names.length;i++){
    var n=names[i],s=sources[n],cls=n===curSrc?'btn on':'btn';
    html+='<button class="'+cls+'" onclick="switchSource(\''+n+'\')">'+s.label+'</button> '}
  html+='<button class="btn" onclick="document.getElementById(\'fileIn\').click()">Load JSON</button>';
  el.innerHTML=html}

function handleFileLoad(evt){
  var f=evt.target.files[0];if(!f)return;
  var r=new FileReader();
  r.onload=function(e){
    try{var j=JSON.parse(e.target.result);
      var name=f.name.replace(/\.json$/i,'').replace(/[^a-zA-Z0-9_]/g,'_');
      registerFileSource(name,f.name.replace(/\.json$/i,''),j);
      switchSource(name)}catch(er){console.error('JSON parse error',er)}};
  r.readAsText(f);evt.target.value=''}

// ═══ BUILT-IN SIMULATOR ═══
var simTime=0,simPhase=-1,simPicks=new Array(10).fill(false);
function simSet(a){simPicks=new Array(10).fill(false);for(var i=0;i<a.length;i++)if(a[i]<10)simPicks[a[i]]=true}
function simGen(dt){
  simTime+=dt;var t=simTime,ph=-1,ped=[0,0,0],lev=[0,0,0,0,0],bar=null,vol=0;
  if(t<1){ph=0;if(simPhase!==ph)simSet([])}
  else if(t<3.5){ph=1;var p=(t-1)/2.5;bar=3+.05*Math.sin(5.2*6.283*t)*sm(Math.min(1,p*3));
    vol=sm(Math.min(1,p*1.2));if(simPhase!==ph)simSet([2,3,4])}
  else if(t<6){ph=2;var p2=(t-3.5)/2.5;bar=3+.04*Math.sin(5*6.283*t);vol=.75+.1*Math.sin(.8*6.283*t);
    if(p2<.2)ped[1]=sm(p2/.2);else if(p2<.7)ped[1]=1;else ped[1]=1-sm((p2-.7)/.25)}
  else if(t<8.5){ph=3;var p3=(t-6)/2.5;
    if(p3<.3)bar=3+5*sm(p3/.3);else bar=8+.05*Math.sin(5.3*6.283*t);
    if(p3>.4&&p3<.8){var pp=(p3-.4)/.4;ped[0]=pp<.4?sm(pp/.4):1-sm((pp-.6)/.4)}
    vol=.8+.08*Math.sin(1.2*6.283*t);if(simPhase!==ph)simSet([2,3,4,5])}
  else if(t<11){ph=4;var p4=(t-8.5)/2.5;
    if(p4<.15)bar=8-3*sm(p4/.15);else bar=5+.04*Math.sin(5.5*6.283*t);
    if(p4<.2)ped[2]=sm(p4/.2);else if(p4<.65)ped[2]=1;else ped[2]=1-sm((p4-.65)/.3);
    vol=.7;if(simPhase!==ph)simSet([3,4,5,7])}
  else if(t<13.5){ph=5;var p5=(t-11)/2.5;
    if(p5<.3)bar=5-2*sm(p5/.3);else bar=3+.05*Math.sin(5*6.283*t);
    if(p5<.15)lev[0]=sm(p5/.15);else if(p5<.65)lev[0]=1;else lev[0]=1-sm((p5-.65)/.25);
    vol=.65+.15*Math.sin(.7*6.283*t);if(simPhase!==ph)simSet([3,4,5,7,9])}
  else if(t<16){ph=6;var p6=(t-13.5)/2.5;
    if(p6<.3)bar=3+4*sm(p6/.3);else bar=7+.04*Math.sin(5.2*6.283*t);
    if(p6<.15)lev[1]=sm(p6/.15);else if(p6<.6)lev[1]=1;else lev[1]=1-sm((p6-.6)/.3);
    vol=.75;if(simPhase!==ph)simSet([2,3,4])}
  else if(t<19){ph=7;var p7=(t-16)/3;
    if(p7<.25)bar=7-4*sm(p7/.25);else if(p7<.5)bar=3+7*sm((p7-.25)/.25);
    else if(p7<.75)bar=10+.04*Math.sin(5.3*6.283*t);else bar=10-7*sm((p7-.75)/.25);
    if(p7>.1&&p7<.55)lev[3]=sm(Math.min(1,(p7-.1)/.1));if(p7>.45)lev[3]=Math.max(0,1-sm((p7-.45)/.1));
    vol=.85;if(simPhase!==ph)simSet([0,2,4,5,7])}
  else if(t<21.5){ph=8;var p8=(t-19)/2.5;bar=3+.06*Math.sin(5.5*6.283*t);
    if(p8<.15)lev[4]=sm(p8/.15);else if(p8<.6)lev[4]=1;else lev[4]=1-sm((p8-.6)/.3);
    if(p8<.2)vol=.85-.5*sm(p8/.2);else if(p8<.5)vol=.35+.55*sm((p8-.2)/.3);else vol=.9;
    if(simPhase!==ph)simSet([1,2,3,4,5])}
  else if(t<23.5){ph=9;var p9=(t-21.5)/2;bar=3+.05*Math.sin(5*6.283*t);vol=.7;
    if(p9<.2)lev[2]=sm(p9/.2);else if(p9<.7)lev[2]=1;else lev[2]=1-sm((p9-.7)/.25)}
  else if(t<27){ph=10;var p10=(t-23.5)/3.5;
    if(p10<.25)bar=3+5*sm(p10/.25);else bar=8+.06*Math.sin(5.2*6.283*t);
    if(p10<.15){ped[0]=sm(p10/.15);ped[1]=sm(p10/.15)}else if(p10<.5){ped[0]=1;ped[1]=1}
    else if(p10<.7){ped[0]=1;ped[1]=1-sm((p10-.5)/.2)}else{ped[0]=1-sm((p10-.7)/.2)}
    if(p10>.2&&p10<.6)lev[0]=sm(Math.min(1,(p10-.2)/.1));if(p10>.5)lev[0]=Math.max(0,1-sm((p10-.5)/.1));
    vol=.85;if(simPhase!==ph)simSet([2,3,4,5,7,9])}
  else if(t<32){ph=11;var p11=(t-27)/5;bar=3+.07*(1-p11)*Math.sin(5*6.283*t);
    vol=.8*(1-sm(Math.max(0,(p11-.1)/.9)))}
  else if(t<35){ph=12;vol=0;bar=null;if(simPhase!==ph)simSet([])}
  else{ph=0;simTime=0;simSet([]);H=[];simPhase=-1;coordReset();return simGen(0)}
  simPhase=ph;
  return{timestamp_us:Math.floor(t*1e6),pedals:ped,levers:lev,bar_sens:sensResp(bar),volume:vol,picks:simPicks.slice()}}

// Register built-in sim
sources['sim']={type:'sim',label:'Demo'};

// ═══ AUDIO ═══
var actx=null,oscs=[],gains=[],masterG=null,soundOn=false,audioStartT=0;
function initAudio(){
  if(actx)return;actx=new(window.AudioContext||window.webkitAudioContext)();
  var irD=2.4,irL=Math.floor(actx.sampleRate*irD),irB=actx.createBuffer(2,irL,actx.sampleRate);
  for(var ch=0;ch<2;ch++){var d=irB.getChannelData(ch);for(var i=0;i<irL;i++){var ti=i/actx.sampleRate;d[i]=(Math.random()*2-1)*Math.exp(-3*ti)*(1+.5*Math.exp(-25*ti))*.3}}
  var rev=actx.createConvolver();rev.buffer=irB;
  var comp=actx.createDynamicsCompressor();comp.threshold.value=-22;comp.knee.value=14;comp.ratio.value=5;comp.attack.value=.008;comp.release.value=.12;
  var lp1=actx.createBiquadFilter();lp1.type='lowpass';lp1.frequency.value=1400;lp1.Q.value=1.2;
  var lp2=actx.createBiquadFilter();lp2.type='lowpass';lp2.frequency.value=4000;lp2.Q.value=.5;
  masterG=actx.createGain();masterG.gain.value=0;
  var dry=actx.createGain();dry.gain.value=.5;var wet=actx.createGain();wet.gain.value=.5;
  for(var i=0;i<OM.length;i++){var osc=actx.createOscillator();osc.type='sawtooth';osc.frequency.value=220;
    osc.detune.value=(i%2===0?1:-1)*(1.5+i*.4);var g=actx.createGain();g.gain.value=0;osc.connect(g);g.connect(masterG);osc.start();oscs.push(osc);gains.push(g)}
  masterG.connect(lp1);lp1.connect(lp2);lp2.connect(comp);comp.connect(dry);comp.connect(rev);rev.connect(wet);dry.connect(actx.destination);wet.connect(actx.destination)}
function updateAudio(){
  if(!actx||!soundOn||!S)return;var now=actx.currentTime;
  masterG.gain.setTargetAtTime(S.volume*.15,now,.016);
  for(var i=0;i<OM.length;i++){var hz=S.string_pitches_hz[i],amp=S.string_amp[i];
    if(hz>20&&amp>AMP_FLOOR){oscs[i].frequency.setTargetAtTime(hz,now,.008);
      gains[i].gain.setTargetAtTime(amp*(i<4?1:(i<7?.75:.5)),now,S.attacks[i]?.003:.012)}
    else gains[i].gain.setTargetAtTime(0,now,.02)}}
function toggleSound(){
  if(!actx)initAudio();soundOn=!soundOn;var btn=document.getElementById('bsnd');
  if(soundOn){actx.resume();btn.innerHTML='&#x1f50a; Sound';btn.classList.add('snd');audioStartT=actx.currentTime-simTime}
  else{if(masterG)masterG.gain.setTargetAtTime(0,actx.currentTime,.04);btn.innerHTML='&#x1f507; Sound';btn.classList.remove('snd')}}

// ═══ WS ═══
var wsConn=null,fc=0,ft=0,df=0;
function toggleW(){if(wsConn){wsConn.close();wsConn=null;document.getElementById('bw').classList.remove('on');return}
  try{wsConn=new WebSocket('ws://'+(location.host||'localhost:8080'));
    wsConn.onopen=function(){curSrc='ws';document.getElementById('bw').classList.add('on');H=[];coordReset();rebuildSourceUI()};
    wsConn.onmessage=function(e){try{var d=JSON.parse(e.data);
      if(d.bar_sens!==undefined&&d.picks!==undefined){
        var pkt={timestamp_us:d.timestamp_us||0,pedals:d.pedals||[0,0,0],levers:d.levers||[0,0,0,0,0],
          bar_sens:d.bar_sens||[0,0,0,0],volume:d.volume!=null?d.volume:0,picks:d.picks?d.picks.map(Boolean):new Array(10).fill(false)};
        pushFrame(coordProcess(pkt,.016))}
      else{
        var f={timestamp_us:d.timestamp_us||0,pedals:d.pedals||[0,0,0],knee_levers:d.knee_levers||[0,0,0,0,0],
          volume:d.volume!=null?d.volume:0,bar_position:d.bar_position!=null?d.bar_position:null,
          bar_confidence:d.bar_confidence||0,bar_source:d.bar_source||'None',bar_sensors:d.bar_sensors||[0,0,0,0],
          string_pitches_hz:d.string_pitches_hz||new Array(10).fill(0),string_active:d.string_active?d.string_active.map(Boolean):new Array(10).fill(false),
          attacks:d.attacks?d.attacks.map(Boolean):new Array(10).fill(false),string_amp:d.string_amp||new Array(10).fill(0)};
        for(var i=0;i<OM.length;i++)if(f.attacks&&f.attacks[i])atkFlash[i]=0.4;pushFrame(f)}
      }catch(er){console.error('WS parse',er)}};
    wsConn.onclose=function(){document.getElementById('bw').classList.remove('on');wsConn=null;
      if(curSrc==='ws'){curSrc='sim';rebuildSourceUI()}};
    wsConn.onerror=function(){wsConn.close()}}catch(e){console.error('WS',e)}}

// ═══ INSTRUMENT VIEW — flat layout ═══
function fretPx(f,fL,fR){var p=fretToPhysical(f);p=parallax(p);return fL+(fR-fL)*p}

function drawInstrument(){
  var c=document.getElementById('instrument');if(!c)return;
  var w=c.clientWidth,h=c.clientHeight;
  if(c.width!==w*2||c.height!==h*2){c.width=w*2;c.height=h*2}
  var x=c.getContext('2d');x.setTransform(2,0,0,2,0,0);x.globalAlpha=1;x.clearRect(0,0,w,h);
  x.fillStyle='#06060e';x.fillRect(0,0,w,h);
  if(!S)return;
  var nStr=OM.length;

  // Layout: [pegs 24px][fretboard][pickup 50px][names 42px] | [controls 88px at bottom]
  var ctrlH=88, fY=16, fBotY=h-ctrlH-4;
  var fH=fBotY-fY; // fretboard height — most of the canvas
  var fL=28, fR=w-96; // left/right edges of fretboard
  var pkL=fR+6, pkR=w-52; // pickup zone
  var nmL=w-48; // note name column

  // Fretboard background
  x.fillStyle='#0d0d1a';x.fillRect(fL-2,fY,fR-fL+4,fH);

  // Nut
  x.fillStyle='#888';x.fillRect(fL-3,fY,4,fH);

  // Fret lines
  for(var f=1;f<=22;f++){var fx=fretPx(f,fL,fR);if(fx>fR)break;
    x.strokeStyle=f<=12?'#505070':'#383850';x.lineWidth=f%5===0?1.8:(f<=12?1:.6);
    x.beginPath();x.moveTo(fx,fY);x.lineTo(fx,fY+fH);x.stroke();
    if(f<=15&&(f%3===0||f===1||f===5||f===7||f===12)){
      x.fillStyle='#777';x.font='7px IBM Plex Mono';x.textAlign='center';
      x.fillText(String(f),fx,fY-1)}}

  // Fret markers
  [3,5,7,9].forEach(function(f){var fx=fretPx(f-.5,fL,fR);
    x.fillStyle='#2a2a48';x.beginPath();x.arc(fx,fY+fH/2,4,0,Math.PI*2);x.fill()});
  var fx12=fretPx(11.5,fL,fR);
  x.fillStyle='#2a2a48';x.beginPath();x.arc(fx12,fY+fH*.35,3,0,Math.PI*2);x.fill();
  x.beginPath();x.arc(fx12,fY+fH*.65,3,0,Math.PI*2);x.fill();

  // Sensor indicators along bottom of fretboard
  for(var si=0;si<SFP.length;si++){
    var sfx=fretPx(SFP[si],fL,fR),sVal=S.bar_sensors[si]||0;
    if(sVal>.05){
      x.fillStyle='#27ae60';x.globalAlpha=sVal*.6;
      x.beginPath();x.arc(sfx,fY+fH-4,4,0,Math.PI*2);x.fill();
      x.globalAlpha=sVal*.12;x.beginPath();x.arc(sfx,fY+fH-4,12,0,Math.PI*2);x.fill();
      x.globalAlpha=1}
    else{x.fillStyle='#222';x.font='5px IBM Plex Mono';x.textAlign='center';x.fillText('f'+SFP[si],sfx,fY+fH+7)}}

  // Strings — full height, widely spaced
  var sPad=8; // top/bottom padding inside fretboard
  for(var i=0;i<nStr;i++){
    var sy=fY+sPad+(fH-2*sPad)*(i/(nStr-1));
    // Vibrating string glow
    if(S.string_amp[i]>AMP_FLOOR){
      x.strokeStyle=SC[i%SC.length];x.globalAlpha=S.string_amp[i]*.3;x.lineWidth=S.string_amp[i]*6;
      x.beginPath();x.moveTo(fL,sy);x.lineTo(fR,sy);x.stroke();x.globalAlpha=1}
    // String line
    x.strokeStyle=SC[i%SC.length];x.globalAlpha=S.string_active[i]?.6:.15;x.lineWidth=S.string_active[i]?1.2:.5;
    x.beginPath();x.moveTo(fL-4,sy);x.lineTo(fR+4,sy);x.stroke();x.globalAlpha=1;

    // String number (left)
    x.fillStyle=S.string_active[i]?SC[i%SC.length]:'#555';x.font='bold 8px IBM Plex Mono';x.textAlign='right';
    x.fillText(String(i+1),fL-8,sy+3);

    // Note names (right): fixed open name always, sounding pitch only when vibrating
    var openName=CFG.copedant.open_notes[i]||'';
    // Fixed open name (rightmost, always visible, dim)
    x.fillStyle='#444';x.font='6px IBM Plex Mono';x.textAlign='right';
    x.fillText(openName,w-2,sy+2);
    // Sounding pitch (only when string has energy)
    if(S.string_amp[i]>AMP_FLOOR){
      var hz=S.string_pitches_hz[i],sounding=m2n(h2m(hz));
      x.fillStyle=SC[i%SC.length];x.font='bold 7px IBM Plex Mono';x.textAlign='left';
      x.fillText(sounding,nmL,sy+2.5)}}

  // Pickup — tall column matching string spread
  x.fillStyle='#141422';x.strokeStyle='#2a2a3a';x.lineWidth=1;
  var pkW=pkR-pkL;
  x.fillRect(pkL,fY+sPad-6,pkW,fH-2*sPad+12);x.strokeRect(pkL,fY+sPad-6,pkW,fH-2*sPad+12);
  for(var i=0;i<nStr;i++){
    var sy=fY+sPad+(fH-2*sPad)*(i/(nStr-1));
    var amp=S.string_amp[i],flash=atkFlash[i]||0;
    var pCx=pkL+pkW/2;
    // Pole piece
    x.fillStyle='#333';x.beginPath();x.arc(pCx,sy,2.5,0,Math.PI*2);x.fill();
    // Energy glow
    if(amp>AMP_FLOOR||flash>0){
      var intensity=Math.max(amp,flash*2);
      x.fillStyle=SC[i%SC.length];x.globalAlpha=intensity*.7;
      x.beginPath();x.arc(pCx,sy,3+intensity*5,0,Math.PI*2);x.fill();
      if(flash>0){x.globalAlpha=flash*.8;x.beginPath();x.arc(pCx,sy,8+flash*10,0,Math.PI*2);x.fill()}
      x.globalAlpha=1}}

  // Bar position
  if(S.bar_position!==null){var bx=fretPx(S.bar_position,fL,fR);
    x.strokeStyle='#1abc9c';x.lineWidth=5;x.lineCap='round';x.globalAlpha=.85;
    x.beginPath();x.moveTo(bx,fY);x.lineTo(bx,fY+fH);x.stroke();
    x.globalAlpha=.15;x.shadowColor='#1abc9c';x.shadowBlur=16;
    x.beginPath();x.moveTo(bx,fY);x.lineTo(bx,fY+fH);x.stroke();
    x.shadowBlur=0;x.globalAlpha=1;x.lineCap='butt';
    x.fillStyle='#1abc9c';x.font='bold 12px IBM Plex Mono';x.textAlign='center';
    x.fillText(S.bar_position.toFixed(1),bx,fY-2)}

  // ── Controls zone at bottom: analog pedals/levers + sparklines ──
  var ctrlH=88, cY=h-ctrlH;
  x.fillStyle='#08080f';x.fillRect(0,cY,w,ctrlH);
  x.strokeStyle='#1a1a2a';x.lineWidth=1;x.beginPath();x.moveTo(0,cY);x.lineTo(w,cY);x.stroke();

  var nPed=PC.length,nLev=LC.length;
  var nCtrl=nPed+nLev+1; // +volume
  var ctrlW=Math.min(70,Math.floor((w-20)/nCtrl-6));
  var gap=6;
  var totalW=nCtrl*(ctrlW+gap)-gap;
  var cx0=Math.round((w-totalW)/2);

  // Pedal/lever analog zone (top portion of controls)
  var analogY=cY+4, analogH=42;
  // Sparkline zone (below analog) — compact
  var sparkY=analogY+analogH+2, sparkH=ctrlH-analogH-10;

  function drawSparkline(sx, sparkData, color, colorHi){
    if(!sparkData||sparkData.length<2)return;
    var sL=sx+2, sR=sx+ctrlW-2, sW=sR-sL;
    x.fillStyle='#0a0a16';x.beginPath();x.roundRect(sL-1,sparkY,sW+2,sparkH,2);x.fill();
    x.strokeStyle='#161628';x.lineWidth=.5;
    x.beginPath();x.moveTo(sL,sparkY+sparkH/2);x.lineTo(sR,sparkY+sparkH/2);x.stroke();
    x.strokeStyle=color;x.lineWidth=1.2;x.globalAlpha=.7;
    x.beginPath();var started=false;
    for(var j=0;j<sparkData.length;j++){
      var px=sL+sW*(j/(sparkData.length-1));
      var val=typeof sparkData[j]==='number'?sparkData[j]:0;
      var py=sparkY+sparkH*(1-Math.max(0,Math.min(1,val)));
      if(!started){x.moveTo(px,py);started=true}else x.lineTo(px,py)}
    x.stroke();
    if(started){x.lineTo(sR,sparkY+sparkH);x.lineTo(sL,sparkY+sparkH);x.closePath();
      x.fillStyle=color;x.globalAlpha=.1;x.fill()}
    x.globalAlpha=1;
    var curVal=sparkData[sparkData.length-1];
    if(typeof curVal==='number'&&curVal>.02){
      var dotY=sparkY+sparkH*(1-curVal);
      x.fillStyle=colorHi;x.beginPath();x.arc(sR,dotY,2.5,0,Math.PI*2);x.fill()}}

  // mode: 'down' (pedals/vol), 'up' (LKV), 'left', 'right' (lateral levers)
  function drawCtrlSlot(idx, label, eng, color, colorHi, sparkData, mode){
    var sx=cx0+idx*(ctrlW+gap);
    var barX=sx+2, barW=ctrlW-4, barH=analogH-8, barY0=analogY+4;
    var dep=Math.max(0,Math.min(1,eng));
    mode=mode||'down';

    // Well/slot
    x.fillStyle='#0c0c18';x.beginPath();x.roundRect(barX-1,barY0-1,barW+2,barH+2,3);x.fill();
    x.strokeStyle='#222';x.lineWidth=.5;x.stroke();

    if(mode==='down'){
      // Pedal face slides DOWN from top
      var slideTop=barY0+dep*(barH-10);
      var pg=x.createLinearGradient(barX,slideTop,barX,slideTop+10);
      pg.addColorStop(0,dep>.4?colorHi:'#777');pg.addColorStop(.3,dep>.4?colorHi:'#aaa');
      pg.addColorStop(.7,dep>.4?color:'#888');pg.addColorStop(1,dep>.4?color:'#555');
      x.fillStyle=pg;x.beginPath();x.roundRect(barX,slideTop,barW,10,2);x.fill();
      for(var g=0;g<2;g++){x.strokeStyle='rgba(0,0,0,.3)';x.lineWidth=.6;
        x.beginPath();x.moveTo(barX+3,slideTop+3+g*3);x.lineTo(barX+barW-3,slideTop+3+g*3);x.stroke()}
      if(dep>.02){x.fillStyle=color;x.globalAlpha=dep*.3;
        x.fillRect(barX+1,slideTop+10,barW-2,barY0+barH-slideTop-10);x.globalAlpha=1}
    } else if(mode==='up'){
      // Vertical lever face slides UP from bottom
      var slideBot=barY0+barH-dep*(barH-10);
      var pg=x.createLinearGradient(barX,slideBot-10,barX,slideBot);
      pg.addColorStop(0,dep>.4?color:'#555');pg.addColorStop(.3,dep>.4?color:'#888');
      pg.addColorStop(.7,dep>.4?colorHi:'#aaa');pg.addColorStop(1,dep>.4?colorHi:'#777');
      x.fillStyle=pg;x.beginPath();x.roundRect(barX,slideBot-10,barW,10,2);x.fill();
      for(var g=0;g<2;g++){x.strokeStyle='rgba(0,0,0,.3)';x.lineWidth=.6;
        x.beginPath();x.moveTo(barX+3,slideBot-7+g*3);x.lineTo(barX+barW-3,slideBot-7+g*3);x.stroke()}
      if(dep>.02){x.fillStyle=color;x.globalAlpha=dep*.3;
        x.fillRect(barX+1,barY0,barW-2,slideBot-10-barY0);x.globalAlpha=1}
    } else {
      // Horizontal: lever face slides LEFT or RIGHT
      var faceW=10, faceH=barH;
      var cx=barX+barW/2; // center rest position
      var travel=(barW-faceW)/2; // max displacement from center
      var dir=mode==='right'?1:-1;
      var slideX=cx-faceW/2+dep*dir*travel;
      // Track behind lever (colored fill from center to face)
      if(dep>.02){x.fillStyle=color;x.globalAlpha=dep*.25;
        var faceCx=slideX+faceW/2;
        var trkL=Math.min(cx,faceCx), trkW=Math.abs(faceCx-cx);
        x.fillRect(trkL,barY0+1,trkW,faceH-2);x.globalAlpha=1}
      // Lever face
      var pg=x.createLinearGradient(slideX,barY0,slideX+faceW,barY0);
      pg.addColorStop(0,dep>.3?color:'#555');pg.addColorStop(.3,dep>.3?colorHi:'#aaa');
      pg.addColorStop(.7,dep>.3?colorHi:'#888');pg.addColorStop(1,dep>.3?color:'#555');
      x.fillStyle=pg;x.beginPath();x.roundRect(slideX,barY0,faceW,faceH,2);x.fill();
      // Vertical grip lines
      for(var g=0;g<2;g++){x.strokeStyle='rgba(0,0,0,.3)';x.lineWidth=.6;
        x.beginPath();x.moveTo(slideX+3+g*3,barY0+3);x.lineTo(slideX+3+g*3,barY0+faceH-3);x.stroke()}
      // Center tick mark (rest position)
      x.strokeStyle='#333';x.lineWidth=.5;
      x.beginPath();x.moveTo(cx,barY0+barH);x.lineTo(cx,barY0+barH+3);x.stroke();
    }

    // Label
    x.fillStyle=dep>.3?colorHi:'#666';x.font='bold 7px IBM Plex Mono';x.textAlign='center';
    x.fillText(label,sx+ctrlW/2,barY0-2);

    // Sparkline
    drawSparkline(sx, sparkData, color, colorHi)}

  // Lever direction map: derive from lever names
  // LKL/RKL → 'left', LKR/RKR → 'right', LKV → 'up'
  function leverDir(name){
    if(!name)return'down';
    var n=name.toUpperCase();
    if(n.indexOf('KV')>=0)return'up';
    if(n.indexOf('KL')>=0&&n.charAt(0)==='L')return'left';   // LKL
    if(n.indexOf('KR')>=0&&n.charAt(0)==='L')return'right';  // LKR
    if(n.indexOf('KL')>=0&&n.charAt(0)==='R')return'left';   // RKL
    if(n.indexOf('KR')>=0&&n.charAt(0)==='R')return'right';  // RKR
    return'down'}

  // Draw pedals (vertical, push down)
  for(var i=0;i<nPed;i++){
    var sparkArr=ctrlHist.pedals.map(function(p){return p[i]||0});
    drawCtrlSlot(i,PNM[i]||('P'+(i+1)),S.pedals[i]||0,'#c07020','#e8a040',sparkArr,'down')}

  // Draw levers (direction from name)
  for(var i=0;i<nLev;i++){
    var sparkArr=ctrlHist.levers.map(function(l){return l[i]||0});
    var dir=leverDir(LNM[i]);
    drawCtrlSlot(nPed+i,LNM[i]||('L'+(i+1)),S.knee_levers[i]||0,'#2060a0','#60a0e0',sparkArr,dir)}

  // Draw volume (vertical, push down)
  var volSparkArr=ctrlHist.volume.slice();
  drawCtrlSlot(nPed+nLev,'VOL',S.volume,'#502890','#a060d0',volSparkArr,'down');

  // Sensor fusion info (bottom-left)
  x.fillStyle={None:'#333',Sensor:'#27ae60',Fused:'#1abc9c'}[S.bar_source]||'#333';
  x.font='6px IBM Plex Mono';x.textAlign='left';
  x.fillText(S.bar_source+' '+Math.round(S.bar_confidence*100)+'%',4,h-3);
}

// ═══ TIMELINE ═══
function tlSetup(canvasId,fixedH){
  var c=document.getElementById(canvasId);if(!c)return null;
  var w=c.clientWidth,h=fixedH||c.clientHeight;
  if(c.width!==w*2||c.height!==h*2){c.width=w*2;c.height=h*2;c.style.height=h+'px'}
  var ctx=c.getContext('2d');ctx.setTransform(2,0,0,2,0,0);ctx.globalAlpha=1;ctx.clearRect(0,0,w,h);
  ctx.fillStyle='#06060e';ctx.fillRect(0,0,w,h);
  var cX=Math.round(w*3/4),hw=cX,wU=RS*1e6;
  var nU=H.length>0?H[H.length-1].timestamp_us:0;
  return{c:c,w:w,h:h,x:ctx,cX:cX,hw:hw,nU:nU,wU:wU}}
function tlX(age,t){return t.cX-t.hw*(age/t.wU)}

// ═══ STAFF — white background, black ink ═══
var DM=[0,0,1,2,2,3,3,4,4,5,6,6],shPC={1:1,6:1,8:1},flPC={3:1,10:1},NHA=-20*Math.PI/180;
var STAFF_H=130;

function drawStaff(){
  var t=tlSetup('staffNotation',STAFF_H);if(!t)return;
  var x=t.x,w=t.w,h=t.h;
  var ls=10,tT=10,bT=tT+6*ls;
  var mCY=tT+5*ls;
  var dS=ls/2;
  var nhA=ls*.42,nhB=nhA*1.5;
  // White background
  x.fillStyle='#ffffff';x.fillRect(0,0,w,h);
  // Staff lines
  x.strokeStyle='#000';x.lineWidth=.8;
  for(var i=0;i<5;i++){
    x.beginPath();x.moveTo(0,tT+i*ls);x.lineTo(w,tT+i*ls);x.stroke();
    x.beginPath();x.moveTo(0,bT+i*ls);x.lineTo(w,bT+i*ls);x.stroke()}
  // Unicode clefs — sized and positioned to staff
  // Treble clef (𝄞 U+1D11E): G-curl wraps 2nd line from bottom = tT+3*ls
  // The glyph baseline sits near bottom of curl; fillText Y = G line + small offset
  x.fillStyle='#000';x.textAlign='left';x.textBaseline='alphabetic';
  x.font=(ls*2.88)+'px Bravura,"Noto Music",serif';
  x.fillText('\u{1D11E}',2,tT+ls*3.1);
  // Bass clef (𝄢 U+1D122): dots flank 2nd line from top = bT+ls
  x.font=(ls*2.8)+'px Bravura,"Noto Music",serif';
  x.fillText('\u{1D122}',2,bT+ls*1.15);

  function m2y(mi){var ri=Math.round(mi),oct=Math.floor(ri/12),pc=((ri%12)+12)%12;
    return mCY-(oct*7+DM[pc]-35)*dS}
  if(H.length<2){
    x.fillStyle='rgba(0,0,0,.1)';x.beginPath();x.moveTo(t.cX-4,2);x.lineTo(t.cX+4,2);x.lineTo(t.cX,8);x.closePath();x.fill();
    
    x.globalAlpha=1;x.setTransform(1,0,0,1,0,0);return}
  // Sustain lines
  for(var si=0;si<OM.length;si++){
    var col=SC[si%SC.length],pPx=null,pY=null,pAmp=0;
    for(var hi=0;hi<H.length;hi++){
      var fr=H[hi],age=t.nU-fr.timestamp_us;if(age>t.wU||age<0){pPx=null;continue}
      var amp=fr.string_amp?fr.string_amp[si]:0;
      if(amp<AMP_FLOOR){pPx=null;pAmp=0;continue}
      var hz=fr.string_pitches_hz[si];if(hz<20){pPx=null;continue}
      var y=m2y(h2m(hz)),px=tlX(age,t);
      if(pPx!==null&&pAmp>AMP_FLOOR){
        x.strokeStyle=col;x.globalAlpha=Math.min(.7,amp*.8);x.lineWidth=1.2+amp*1.5;
        x.beginPath();x.moveTo(pPx,pY);x.lineTo(px,y);x.stroke()}
      pPx=px;pY=y;pAmp=amp}}
  // Noteheads
  var evts=[];
  for(var si=0;si<OM.length;si++){for(var hi=0;hi<H.length;hi++){
    var fr=H[hi];if(!(fr.attacks&&fr.attacks[si]))continue;
    var age=t.nU-fr.timestamp_us;if(age>t.wU||age<0)continue;
    var hz=fr.string_pitches_hz[si];if(hz<20)continue;
    evts.push({si:si,age:age,hz:hz,fr:fr})}}
  evts.sort(function(a,b){return b.age-a.age});
  var lastAcc={};
  for(var ei=0;ei<evts.length;ei++){
    var ev=evts[ei],col=SC[ev.si],fr=ev.fr,age=ev.age;
    var mi=h2m(ev.hz),ri=Math.round(mi),pc=((ri%12)+12)%12;
    var y=m2y(mi),px=tlX(age,t);
    if(y<tT-4*ls||y>bT+8*ls)continue;
    var fA=1-.1*(age/t.wU),vA=Math.max(.5,.3+.7*fr.volume),lH=nhB+3;
    x.strokeStyle='#000';x.lineWidth=.8;x.globalAlpha=vA*fA;
    if(Math.abs(y-mCY)<1){x.beginPath();x.moveTo(px-lH,mCY);x.lineTo(px+lH,mCY);x.stroke()}
    for(var ly=tT-ls;ly>=y-1;ly-=ls){x.beginPath();x.moveTo(px-lH,ly);x.lineTo(px+lH,ly);x.stroke()}
    for(var ly2=bT+4*ls+ls;ly2<=y+1;ly2+=ls){x.beginPath();x.moveTo(px-lH,ly2);x.lineTo(px+lH,ly2);x.stroke()}
    x.fillStyle=col;x.globalAlpha=vA*fA;x.beginPath();
    for(var st=0;st<24;st++){var t2=st/24*Math.PI*2;
      var ex=nhB*Math.cos(t2),ey=nhA*Math.sin(t2);
      var rx=ex*Math.cos(NHA)-ey*Math.sin(NHA),ry=ex*Math.sin(NHA)+ey*Math.cos(NHA);
      if(st===0)x.moveTo(px+rx,y+ry);else x.lineTo(px+rx,y+ry)}
    x.closePath();x.fill();x.strokeStyle='rgba(0,0,0,.2)';x.lineWidth=.5;x.stroke();
    var oct=Math.floor(ri/12),diaStep=oct*7+DM[pc];
    var thisAcc=shPC[pc]?'sharp':flPC[pc]?'flat':'natural';
    var prev=lastAcc[diaStep],show=null;
    if(!prev){if(thisAcc==='sharp')show='\u266F';else if(thisAcc==='flat')show='\u266D'}
    else if(prev!==thisAcc){show=thisAcc==='sharp'?'\u266F':thisAcc==='flat'?'\u266D':'\u266E'}
    lastAcc[diaStep]=thisAcc;
    if(show){x.fillStyle='rgba(0,0,0,.6)';x.globalAlpha=fA;x.font=(ls*1.1)+'px serif';x.textAlign='right';
      x.fillText(show,px-nhB-2,y+ls*.35)}}
  // Playhead line + lead-in zone
  x.globalAlpha=1;var hC=S&&S.bar_position!==null&&S.volume>.02;
  // Lead-in zone (right of playhead): subtle tint + time grid
  var liL=t.cX+1, liW=w-liL;
  x.fillStyle='rgba(240,245,250,.5)';x.fillRect(liL,0,liW,h);
  // Time ticks in lead-in zone
  x.strokeStyle='rgba(0,0,0,.04)';x.lineWidth=.5;
  for(var ts=0.5;ts<RS;ts+=0.5){var tx=t.cX+(t.cX*(ts/RS));if(tx<w){
    x.beginPath();x.moveTo(tx,0);x.lineTo(tx,h);x.stroke()}}
  // Playhead line
  x.strokeStyle=hC?'rgba(26,188,156,.7)':'rgba(0,0,0,.15)';x.lineWidth=1.5;
  x.beginPath();x.moveTo(t.cX,0);x.lineTo(t.cX,h);x.stroke();
  x.fillStyle=hC?'rgba(26,188,156,.6)':'rgba(0,0,0,.1)';
  x.beginPath();x.moveTo(t.cX-5,1);x.lineTo(t.cX+5,1);x.lineTo(t.cX,8);x.closePath();x.fill();
  
  x.globalAlpha=1;x.setTransform(1,0,0,1,0,0)}

// ═══ ENVELOPE ═══
function drawEnvelope(){
  var t=tlSetup('attackStrip',30);if(!t)return;
  var x=t.x,h=t.h;if(H.length<2){
    x.fillStyle='rgba(80,80,100,.4)';x.beginPath();x.moveTo(t.cX-4,0);x.lineTo(t.cX+4,0);x.lineTo(t.cX,6);x.closePath();x.fill();
    x.globalAlpha=1;x.setTransform(1,0,0,1,0,0);return}
  var nStr=OM.length,sh=h/nStr;
  for(var si=0;si<nStr;si++){
    var lY=si*sh,lH=sh-1,col=SC[si%SC.length];
    x.beginPath();var started=false,lastPx=t.cX;
    for(var hi=0;hi<H.length;hi++){
      var fr=H[hi],age=t.nU-fr.timestamp_us;if(age>t.wU||age<0)continue;
      var px=tlX(age,t),amp=fr.string_amp?fr.string_amp[si]:0,ey=lY+lH*(1-amp);
      if(!started){x.moveTo(px,lY+lH);x.lineTo(px,ey);started=true}else x.lineTo(px,ey);lastPx=px}
    if(started){x.lineTo(lastPx,lY+lH);x.closePath();x.fillStyle=col;x.globalAlpha=.4;x.fill()}
    for(var hi=0;hi<H.length;hi++){var fr=H[hi],age=t.nU-fr.timestamp_us;if(age>t.wU||age<0)continue;
      if(fr.attacks&&fr.attacks[si]){var px=tlX(age,t);
        x.strokeStyle=col;x.globalAlpha=.8;x.lineWidth=1.5;
        x.beginPath();x.moveTo(px,lY);x.lineTo(px,lY+lH);x.stroke()}}}
  for(var i=0;i<atkFlash.length;i++){if(atkFlash[i]>0){x.fillStyle=SC[i%SC.length];x.globalAlpha=atkFlash[i]*1.5;x.fillRect(t.cX-2,i*sh,5,sh-1)}}
  // Lead-in zone
  x.globalAlpha=1;x.fillStyle='rgba(255,255,255,.03)';x.fillRect(t.cX+1,0,t.w-t.cX-1,h);
  // Playhead
  x.strokeStyle='rgba(26,188,156,.5)';x.lineWidth=1.5;
  x.beginPath();x.moveTo(t.cX,0);x.lineTo(t.cX,h);x.stroke();
  x.globalAlpha=.3;x.fillStyle='#555';x.font='7px IBM Plex Mono';x.textAlign='right';x.fillText('ENV',t.w-4,h-2);
  x.globalAlpha=1;x.setTransform(1,0,0,1,0,0)}

// ═══ TABLATURE — white background, matching staff notation ═══
// Darker string colors for tab contrast on white
var TC=['#b03030','#b06018','#8a7008','#1a9a40','#109878','#1a70b0','#1a5890','#703898','#6a2e80','#c01850'];
function drawTab(){
  var t=tlSetup('tablature',240);if(!t)return;
  var x=t.x,w=t.w,h=t.h,nS=OM.length,tM=10,bM=14;
  var labelW=52; // right margin for labels
  var sH=(h-tM-bM)/nS;
  // White background
  x.fillStyle='#ffffff';x.fillRect(0,0,w,h);
  // String lines + labels
  for(var i=0;i<nS;i++){var y=tM+i*sH+sH/2;var tc=TC[i%TC.length];
    x.strokeStyle=tc;x.globalAlpha=.2;x.lineWidth=.7;x.beginPath();x.moveTo(0,y);x.lineTo(w,y);x.stroke();x.globalAlpha=1;
    x.fillStyle=tc;x.globalAlpha=.7;x.beginPath();x.arc(5,y,3,0,Math.PI*2);x.fill();x.globalAlpha=1;
    x.fillStyle=tc;x.globalAlpha=.6;x.font='8px IBM Plex Mono';x.textAlign='left';x.fillText(String(i+1),11,y+3);x.globalAlpha=1;
    // Fixed open string name (rightmost)
    var openName=CFG.copedant.open_notes[i]||'';
    x.fillStyle=tc;x.globalAlpha=.55;x.font='8px IBM Plex Mono';x.textAlign='right';
    x.fillText(openName,w-3,y+3);x.globalAlpha=1;
    // Sounding pitch (to left of open name, shown when active and different)
    if(S&&S.string_active[i]){
      var sounding=m2n(h2m(S.string_pitches_hz[i]));
      if(sounding!==openName){
        x.fillStyle=tc;x.globalAlpha=.9;x.font='bold 9px IBM Plex Mono';x.textAlign='right';
        x.fillText(sounding,w-labelW+14,y+3);x.globalAlpha=1}}}
  if(H.length<2){
    x.fillStyle='rgba(0,0,0,.1)';x.beginPath();x.moveTo(t.cX-4,tM);x.lineTo(t.cX+4,tM);x.lineTo(t.cX,tM+6);x.closePath();x.fill();
    x.globalAlpha=1;x.setTransform(1,0,0,1,0,0);return}
  for(var si=0;si<nS;si++){var col=TC[si%TC.length],sY=tM+si*sH+sH/2;
    for(var hi=0;hi<H.length;hi++){var fr=H[hi],age=t.nU-fr.timestamp_us;if(age>t.wU||age<0)continue;
      if(!(fr.attacks&&fr.attacks[si]))continue;
      var fret=fr.bar_position;if(fret===null)continue;
      var px=tlX(age,t),fA=1-.08*(age/t.wU),vA=Math.max(.5,.4+.6*fr.volume);
      var fretStr=String(Math.round(fret));
      // Build standard tab letters (A=P1, B=P2, C=P3) and full pedal/lever names
      var tabLetter='', fullName='';
      if(fr.pedals)for(var p=0;p<fr.pedals.length;p++)if((fr.pedals[p]||0)>.5){
        tabLetter+=(TAB_PED[p]||'');fullName+=(fullName?'+':'')+(PNM[p]||('P'+(p+1)))}
      if(fr.knee_levers)for(var k=0;k<fr.knee_levers.length;k++)if((fr.knee_levers[k]||0)>.5){
        fullName+=(fullName?'+':'')+(LNM[k]||('L'+(k+1)))}
      // Render: fret number + standard letter(s) same size, right-adjacent
      var fontSize=sH*.5;
      x.fillStyle=col;x.globalAlpha=vA*fA;x.font='bold '+fontSize+'px IBM Plex Mono';
      if(tabLetter){
        var fW=x.measureText(fretStr).width, lW=x.measureText(tabLetter).width;
        var totalW=fW+lW+1;
        x.textAlign='left';
        x.fillText(fretStr,px-totalW/2,sY+sH*.12);
        x.fillText(tabLetter,px-totalW/2+fW+1,sY+sH*.12)}
      else{x.textAlign='center';x.fillText(fretStr,px,sY+sH*.12)}
      // Full pedal/lever names (smaller, below)
      if(fullName){x.globalAlpha=vA*fA*.5;x.font=(sH*.26)+'px IBM Plex Mono';x.textAlign='center';
        x.fillText(fullName,px,sY+sH*.38)}}}
  // Playhead line + lead-in zone
  x.globalAlpha=1;var hC=S&&S.bar_position!==null&&S.volume>.02;
  // Lead-in zone (right of playhead)
  var liL=t.cX+1, liW=w-liL;
  x.fillStyle='rgba(240,245,250,.5)';x.fillRect(liL,0,liW,h);
  x.strokeStyle='rgba(0,0,0,.04)';x.lineWidth=.5;
  for(var ts=0.5;ts<RS;ts+=0.5){var tx=t.cX+(t.cX*(ts/RS));if(tx<w){
    x.beginPath();x.moveTo(tx,0);x.lineTo(tx,h);x.stroke()}}
  // Playhead line
  x.strokeStyle=hC?'rgba(26,188,156,.7)':'rgba(0,0,0,.15)';x.lineWidth=1.5;
  x.beginPath();x.moveTo(t.cX,0);x.lineTo(t.cX,h);x.stroke();
  x.fillStyle=hC?'rgba(26,188,156,.6)':'rgba(0,0,0,.1)';
  x.beginPath();x.moveTo(t.cX-5,tM);x.lineTo(t.cX+5,tM);x.lineTo(t.cX,tM+7);x.closePath();x.fill();
  x.globalAlpha=1;x.fillStyle='rgba(0,0,0,.12)';x.font='7px IBM Plex Mono';x.textAlign='right';x.fillText('TAB',w-3,h-3);
  x.globalAlpha=1;x.setTransform(1,0,0,1,0,0)}

// ═══ PIANO ROLL ═══
var ROLL_LW=30,ROLL_RW=24;
function drawRoll(){
  var c=document.getElementById('pianoRoll');if(!c)return;
  var w=c.clientWidth,h=c.clientHeight;if(h<10)return;
  if(c.width!==w*2||c.height!==h*2){c.width=w*2;c.height=h*2}
  var x=c.getContext('2d');x.setTransform(2,0,0,2,0,0);x.globalAlpha=1;x.clearRect(0,0,w,h);
  var mR=MH-ML;
  x.fillStyle='#06060e';x.fillRect(0,0,w,h);
  x.fillStyle='#0a0a12';x.fillRect(0,0,ROLL_LW,h);
  x.strokeStyle='#161628';x.lineWidth=1;x.beginPath();x.moveTo(ROLL_LW,0);x.lineTo(ROLL_LW,h);x.stroke();
  x.fillStyle='#0a0a12';x.fillRect(w-ROLL_RW,0,ROLL_RW,h);
  x.strokeStyle='#161628';x.lineWidth=1;x.beginPath();x.moveTo(w-ROLL_RW,0);x.lineTo(w-ROLL_RW,h);x.stroke();
  var cX=Math.round(w*3/4),hw=cX,wU=RS*1e6;
  for(var m=ML;m<=MH;m++){var y=h-((m-ML)/mR)*h,nn=NN[m%12];
    var isC=nn==='C',isE=nn==='E';
    if(isC||isE){x.strokeStyle=isC?'rgba(255,255,255,.1)':'rgba(255,255,255,.04)';x.lineWidth=isC?.8:.4;
      x.beginPath();x.moveTo(ROLL_LW,y);x.lineTo(w-ROLL_RW,y);x.stroke()}
    if(isC||isE||nn==='G#'||nn==='A'){
      var oct=Math.floor(m/12)-1,hz=440*Math.pow(2,(m-69)/12);
      x.fillStyle=isC?'#99a':'#667';x.font='bold 6px IBM Plex Mono';x.textAlign='right';
      x.fillText(Math.round(hz),ROLL_LW-3,y+2);
      x.fillStyle=isC?'#99a':'#667';x.font='bold 7px IBM Plex Mono';x.textAlign='left';
      x.fillText(nn+oct,w-ROLL_RW+3,y+3)}}
  if(H.length<2){
    x.fillStyle='rgba(80,80,100,.4)';x.beginPath();x.moveTo(cX-4,0);x.lineTo(cX+4,0);x.lineTo(cX,6);x.closePath();x.fill();
    x.globalAlpha=1;x.setTransform(1,0,0,1,0,0);return}
  var nU=H[H.length-1].timestamp_us;
  for(var si=0;si<OM.length;si++){
    var col=SC[si%SC.length],pPx=null,pPy=null,pAmp=0;
    for(var hi=0;hi<H.length;hi++){
      var f=H[hi],age=nU-f.timestamp_us;if(age>wU||age<0){pPx=null;continue}
      var amp=f.string_amp?f.string_amp[si]:0;
      if(amp<AMP_FLOOR){pPx=null;pAmp=0;continue}
      var hz=f.string_pitches_hz[si];if(hz<20){pPx=null;continue}
      var mi=h2m(hz);if(mi<ML||mi>MH){pPx=null;continue}
      var px=cX-hw*(age/wU);if(px<ROLL_LW){pPx=null;continue}
      var py=h-((mi-ML)/mR)*h;
      var fA=1-.08*(age/wU),lw=1+amp*2.5,alpha=Math.min(1,amp*fA*.9);
      if(pPx!==null&&pAmp>AMP_FLOOR){
        x.strokeStyle=col;x.globalAlpha=alpha;x.lineWidth=lw;
        x.beginPath();x.moveTo(pPx,pPy);x.lineTo(px,py);x.stroke()}
      if(f.attacks&&f.attacks[si]){x.fillStyle=col;x.globalAlpha=Math.min(1,alpha+.3);
        x.beginPath();x.arc(px,py,3,0,Math.PI*2);x.fill()}
      pPx=px;pPy=py;pAmp=amp}}
  x.globalAlpha=1;
  // Lead-in zone
  x.fillStyle='rgba(255,255,255,.03)';x.fillRect(cX+1,0,w-ROLL_RW-cX-1,h);
  // Playhead
  var hC=S&&S.bar_position!==null&&S.volume>.02;
  x.strokeStyle=hC?'rgba(26,188,156,.6)':'rgba(80,80,100,.3)';x.lineWidth=1.5;
  x.beginPath();x.moveTo(cX,0);x.lineTo(cX,h);x.stroke();
  x.fillStyle=hC?'rgba(26,188,156,.9)':'rgba(80,80,100,.4)';
  x.beginPath();x.moveTo(cX-5,0);x.lineTo(cX+5,0);x.lineTo(cX,7);x.closePath();x.fill();
  x.globalAlpha=.3;x.fillStyle='#555';x.font='7px IBM Plex Mono';x.textAlign='right';x.fillText('ROLL',w-ROLL_RW-4,h-2);
  x.globalAlpha=1;x.setTransform(1,0,0,1,0,0)}

// ═══ MAIN LOOP ═══
var prevTime=performance.now();
function mainLoop(now){
  var dt=(now-prevTime)/1000;prevTime=now;if(dt>.1)dt=.016;
  for(var i=0;i<atkFlash.length;i++)if(atkFlash[i]>0)atkFlash[i]=Math.max(0,atkFlash[i]-dt);
  // Source-driven packet generation
  if(curSrc!=='ws'){
    // Audio sync: when sound is on for sim, use audio clock
    if(curSrc==='sim'&&actx&&soundOn){
      simTime=actx.currentTime-audioStartT;
      var pkt=simGen(0);
      if(pkt)pushFrame(coordProcess(pkt,dt))
    }else{
      var wallT=now/1000;
      var pkt=sourceNext(dt,wallT);
      if(pkt)pushFrame(coordProcess(pkt,dt))}}
  updateAudio();pushCtrlHist();fc++;ft+=dt;
  if(ft>=.5){df=Math.round(fc/ft);fc=0;ft=0;document.getElementById('fps').textContent=df+'fps'}
  drawInstrument();drawStaff();drawTab();drawEnvelope();drawRoll();
  requestAnimationFrame(mainLoop)}

var nS=OM.length,nP=PC.length,nL=LC.length;
S={timestamp_us:0,pedals:new Array(nP).fill(0),knee_levers:new Array(nL).fill(0),volume:0,bar_position:null,
  bar_confidence:0,bar_source:'None',bar_sensors:new Array(SFP.length).fill(0),string_pitches_hz:cpd(null,new Array(nP).fill(0),new Array(nL).fill(0)),
  string_active:new Array(nS).fill(false),attacks:new Array(nS).fill(false),string_amp:new Array(nS).fill(0)};

// ═══ BUILT-IN: Gone Country (Paul Franklin / Alan Jackson) ═══
(function(){
  var BPM=130,beat=60000/BPM,rate=60;
  var evts=[
    [0,8,[0,0,0],[0,0,0,0,0],.7,[0,0,1,0,0,0,0,0,0,0]],
    [.5,10,[0,0,0],[0,0,0,0,0],.9,[0,0,1,1,1,1,0,0,0,0]],
    [1,10,[1,0,0],[0,0,0,0,0],.9,[0,0,1,1,1,1,0,0,0,0]],
    [1.5,10,[0,0,0],[0,0,0,0,0],.85,[0,0,1,1,1,1,0,0,0,0]],
    [2,10,[0,0,0],[0,0,0,0,0],.85,[0,0,1,1,1,1,0,0,0,0]],
    [2.5,10,[1,0,0],[0,0,0,0,0],.85,[0,0,1,1,1,1,0,0,0,0]],
    [3,8,[1,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [3.25,6.5,[1,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [3.5,5,[1,0,0],[0,0,0,0,0],.85,[0,0,1,1,1,1,0,0,0,0]],
    [4,5,[0,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [4.5,5,[0,1,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [5,4,[1,0,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [5.25,3.5,[1,0,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [5.5,3,[1,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [6,3,[0,0,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [6.5,3,[0,1,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [7,10,[0,0,0],[0,0,0,0,0],.9,[0,0,1,1,1,1,0,0,0,0]],
    [7.5,10,[1,0,0],[0,0,0,0,0],.9,[0,0,1,1,1,1,0,0,0,0]],
    [8,10,[0,0,0],[0,0,0,0,0],.85,[0,0,1,1,1,1,0,0,0,0]],
    [8.5,10,[1,0,0],[0,0,0,0,0],.85,[0,0,1,1,1,1,0,0,0,0]],
    [9,7.5,[0,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [9.25,5,[0,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [9.5,5,[1,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [10,5,[1,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [10.5,5,[0,1,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [11,3,[0,0,0],[0,0,0,0,0],.8,[0,0,1,1,1,1,0,0,0,0]],
    [11.5,3,[0,1,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [12,3,[1,0,0],[0,0,0,0,0],.75,[0,0,1,1,1,1,0,0,0,0]],
    [12.5,3,[0,0,0],[0,0,0,0,0],.7,[0,0,1,1,1,1,0,0,0,0]],
    [13,3,[0,0,0],[0,0,0,0,0],.6,[0,0,1,1,1,1,0,0,0,0]],
    [14,3,[0,0,0],[0,0,0,0,0],.3,[0,0,0,0,0,0,0,0,0,0]],
    [15,null,[0,0,0],[0,0,0,0,0],0,[0,0,0,0,0,0,0,0,0,0]]
  ];
  var totalMs=15*beat,pkts=[];
  for(var fr=0;fr<Math.ceil(totalMs/(1000/rate));fr++){
    var t_ms=fr*(1000/rate),tb=t_ms/beat;
    var prev=evts[0],next=evts[evts.length-1];
    for(var i=0;i<evts.length-1;i++){if(tb>=evts[i][0]&&tb<evts[i+1][0]){prev=evts[i];next=evts[i+1];break}}
    if(tb>=evts[evts.length-1][0])prev=next=evts[evts.length-1];
    var frac=(next[0]-prev[0])>0?(tb-prev[0])/(next[0]-prev[0]):0;
    var sf=frac*frac*(3-2*frac);
    var bar=prev[1]!==null&&next[1]!==null?prev[1]+(next[1]-prev[1])*sf:prev[1];
    var ped=prev[2].map(function(v,i){return v+(next[2][i]-v)*sf});
    var lev=prev[3].map(function(v,i){return v+(next[3][i]-v)*sf});
    var vol=prev[4]+(next[4]-prev[4])*sf;
    var picks=prev[5].map(Boolean);
    if(bar!==null)bar+=.04*Math.sin(5.5*6.283*t_ms/1000)*vol;
    pkts.push({t_us:Math.floor(t_ms*1000),
      sens:bar!==null?sensResp(bar):[0,0,0,0],
      ped:ped,lev:lev,vol:Math.round(vol*1000)/1000,
      picks:picks})}
  registerFileSource('gc','Gone Country',{packets:pkts,sample_rate_hz:rate,duration_s:totalMs/1000});
})();

switchSource('gc');
requestAnimationFrame(mainLoop);
