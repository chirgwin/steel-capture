#!/usr/bin/env node
// ═══ Steel Capture Visualization Unit Tests ═══
// Run: node test_viz.js

// Stub browser globals so viz.js can be evaluated
var document={getElementById:function(){return null}};
var performance={now:function(){return 0}};
function requestAnimationFrame(){}
var window={AudioContext:null,webkitAudioContext:null};

// Load viz.js
eval(require('fs').readFileSync(__dirname+'/viz.js','utf8'));

// ── Test framework ──
var pass=0,fail=0,section='';
function describe(name){section=name;console.log('\n=== '+name+' ===')}
function eq(a,b,msg){
  if(a===b){pass++;console.log('  ✓ '+msg)}
  else{fail++;console.log('  ✗ '+msg+' | got: '+JSON.stringify(a)+' expected: '+JSON.stringify(b))}}
function near(a,b,tol,msg){
  if(Math.abs(a-b)<(tol||0.01)){pass++}
  else{fail++;console.log('  ✗ '+msg+' | got: '+a+' expected: '+b+' (±'+tol+')')}}

// ── 1. Open string MIDI notes ──
describe('Open string MIDI notes');
var expected=[
  {s:1,note:'F#4',midi:66},{s:2,note:'D#4',midi:63},{s:3,note:'G#4',midi:68},
  {s:4,note:'E4',midi:64},{s:5,note:'B3',midi:59},{s:6,note:'G#3',midi:56},
  {s:7,note:'F#3',midi:54},{s:8,note:'E3',midi:52},{s:9,note:'D3',midi:50},
  {s:10,note:'B2',midi:47}
];
for(var i=0;i<expected.length;i++){
  eq(OM[i],expected[i].midi,'String '+expected[i].s+' = MIDI '+expected[i].midi+' ('+expected[i].note+')');}

// ── 2. Open string pitch names via cpd() ──
describe('Open string pitch names (cpd → h2m → m2n)');
var hzOpen=cpd(null,[0,0,0],[0,0,0,0,0]);
for(var i=0;i<expected.length;i++){
  eq(m2n(h2m(hzOpen[i])),expected[i].note,'String '+expected[i].s+' → '+expected[i].note);}

// ── 3. m2n note name mapping ──
describe('m2n note name mapping');
eq(m2n(60),'C4','MIDI 60');eq(m2n(61),'C#4','MIDI 61');eq(m2n(62),'D4','MIDI 62');
eq(m2n(63),'D#4','MIDI 63');eq(m2n(64),'E4','MIDI 64');eq(m2n(65),'F4','MIDI 65');
eq(m2n(66),'F#4','MIDI 66');eq(m2n(67),'G4','MIDI 67');eq(m2n(68),'G#4','MIDI 68');
eq(m2n(69),'A4','MIDI 69');eq(m2n(70),'A#4','MIDI 70');eq(m2n(71),'B4','MIDI 71');
eq(m2n(47),'B2','MIDI 47');eq(m2n(48),'C3','MIDI 48');eq(m2n(72),'C5','MIDI 72');

// ── 4. h2m round-trip ──
describe('h2m/Hz round-trip');
for(var m=40;m<=90;m++){
  var hz=440*Math.pow(2,(m-69)/12);
  near(h2m(hz),m,0.001,'MIDI '+m);}

// ── 5. Pedal changes ──
describe('Pedal P1: strings 5,10 → +2 semitones');
var hzP1=cpd(null,[1,0,0],[0,0,0,0,0]);
eq(m2n(h2m(hzP1[4])),'C#4','Str 5: B3→C#4');
eq(m2n(h2m(hzP1[9])),'C#3','Str 10: B2→C#3');
eq(m2n(h2m(hzP1[0])),'F#4','Str 1 unchanged');
eq(m2n(h2m(hzP1[2])),'G#4','Str 3 unchanged');

describe('Pedal P2: strings 3,6 → +1 semitone');
var hzP2=cpd(null,[0,1,0],[0,0,0,0,0]);
eq(m2n(h2m(hzP2[2])),'A4','Str 3: G#4→A4');
eq(m2n(h2m(hzP2[5])),'A3','Str 6: G#3→A3');
eq(m2n(h2m(hzP2[3])),'E4','Str 4 unchanged');

describe('Pedal P3: strings 4,5 → +2 semitones');
var hzP3=cpd(null,[0,0,1],[0,0,0,0,0]);
eq(m2n(h2m(hzP3[3])),'F#4','Str 4: E4→F#4');
eq(m2n(h2m(hzP3[4])),'C#4','Str 5: B3→C#4');

// ── 6. Lever changes ──
describe('Lever LKL1: strings 4,8 → +1 semitone');
var hzLKL=cpd(null,[0,0,0],[1,0,0,0,0]);
eq(m2n(h2m(hzLKL[3])),'F4','Str 4: E4→F4');
eq(m2n(h2m(hzLKL[7])),'F3','Str 8: E3→F3');

describe('Lever LKV: strings 5,10 → −1 semitone');
var hzLKV=cpd(null,[0,0,0],[0,1,0,0,0]);
eq(m2n(h2m(hzLKV[4])),'A#3','Str 5: B3→A#3');
eq(m2n(h2m(hzLKV[9])),'A#2','Str 10: B2→A#2');

describe('Lever LKR: strings 4,8 → −1 semitone');
var hzLKR=cpd(null,[0,0,0],[0,0,1,0,0]);
eq(m2n(h2m(hzLKR[3])),'D#4','Str 4: E4→D#4');
eq(m2n(h2m(hzLKR[7])),'D#3','Str 8: E3→D#3');

describe('Lever RKL: strings 1(+2), 2(+1), 7(+2)');
var hzRKL=cpd(null,[0,0,0],[0,0,0,1,0]);
eq(m2n(h2m(hzRKL[0])),'G#4','Str 1: F#4→G#4');
eq(m2n(h2m(hzRKL[1])),'E4','Str 2: D#4→E4');
eq(m2n(h2m(hzRKL[6])),'G#3','Str 7: F#3→G#3');

describe('Lever RKR: strings 2(−1), 6(−2), 9(−1)');
var hzRKR=cpd(null,[0,0,0],[0,0,0,0,1]);
eq(m2n(h2m(hzRKR[1])),'D4','Str 2: D#4→D4');
eq(m2n(h2m(hzRKR[5])),'F#3','Str 6: G#3→F#3');
eq(m2n(h2m(hzRKR[8])),'C#3','Str 9: D3→C#3');

// ── 7. Combined pedals+levers ──
describe('Combined: P1+P2 (common Nashville chord)');
var hzP12=cpd(null,[1,1,0],[0,0,0,0,0]);
eq(m2n(h2m(hzP12[2])),'A4','P2: Str 3 G#4→A4');
eq(m2n(h2m(hzP12[4])),'C#4','P1: Str 5 B3→C#4');
eq(m2n(h2m(hzP12[5])),'A3','P2: Str 6 G#3→A3');
eq(m2n(h2m(hzP12[9])),'C#3','P1: Str 10 B2→C#3');

describe('Combined: P1+LKV conflict on strings 5,10 (P1 +2, LKV −1 → net +1)');
var hzP1V=cpd(null,[1,0,0],[0,1,0,0,0]);
eq(m2n(h2m(hzP1V[4])),'C4','Str 5: B3+2−1→C4');
eq(m2n(h2m(hzP1V[9])),'C3','Str 10: B2+2−1→C3');

// ── 8. Fret transposition ──
describe('Fret transposition');
var hzF0=cpd(0,[0,0,0],[0,0,0,0,0]);
var hzF12=cpd(12,[0,0,0],[0,0,0,0,0]);
for(var i=0;i<10;i++)
  near(hzF12[i]/hzF0[i],2.0,0.001,'Str '+(i+1)+' fret 12 = octave');
var hzF5=cpd(5,[0,0,0],[0,0,0,0,0]);
for(var i=0;i<10;i++)
  near(hzF5[i]/hzF0[i],Math.pow(2,5/12),0.001,'Str '+(i+1)+' fret 5 ratio');

describe('Fret + pedal combined');
var hzF3P1=cpd(3,[1,0,0],[0,0,0,0,0]);
// String 5: B3 open + P1(+2) = C#4 + 3 frets = E4
eq(m2n(h2m(hzF3P1[4])),'E4','Str 5: B3+P1+fret3 → E4');
// String 1: F#4 + 3 frets = A4
eq(m2n(h2m(hzF3P1[0])),'A4','Str 1: F#4+fret3 → A4');

// ── 9. Staff Y-position mapping ──
describe('Staff geometry: note → Y position');
// Reconstruct staff constants as in drawStaff
var ls=10,tT=10,bT=tT+6*ls,mCY=tT+5*ls,dS=ls/2;
var DM=[0,0,1,2,2,3,3,4,4,5,6,6];
function m2y(mi){var ri=Math.round(mi),oct=Math.floor(ri/12),pc=((ri%12)+12)%12;
  return mCY-(oct*7+DM[pc]-35)*dS}

// Treble staff lines (top to bottom): F5, D5, B4, G4, E4
eq(m2y(77),tT,         'F5 → top treble line (tT)');
eq(m2y(74),tT+ls,      'D5 → 2nd treble line');
eq(m2y(71),tT+2*ls,    'B4 → 3rd treble line');
eq(m2y(67),tT+3*ls,    'G4 → 4th treble line (G clef)');
eq(m2y(64),tT+4*ls,    'E4 → bottom treble line');

// Middle C (ledger line between staves)
eq(m2y(60),mCY,        'C4 → middle C (mCY)');

// Bass staff lines (top to bottom): A3, F3, D3, B2, G2
eq(m2y(57),bT,         'A3 → top bass line');
eq(m2y(53),bT+ls,      'F3 → 2nd bass line (F clef)');
eq(m2y(50),bT+2*ls,    'D3 → 3rd bass line');
eq(m2y(47),bT+3*ls,    'B2 → 4th bass line');
eq(m2y(43),bT+4*ls,    'G2 → bottom bass line');

// Accidentals between lines
eq(m2y(65),tT+4*ls-dS, 'F4 → one step above E4');
eq(m2y(69),tT+2*ls+dS, 'A4 → one step below B4');
eq(m2y(72),tT+ls+dS,   'C5 → one step below D5');

// ── 10. Staff: all open strings land correctly ──
describe('Staff positions for all open strings');
var openStrs=[
  {s:1,midi:66,name:'F#4',between:'E4-G4'},  // between lines 5 and 4
  {s:2,midi:63,name:'D#4',between:'E4 space below'},
  {s:3,midi:68,name:'G#4',between:'G4-B4'},
  {s:4,midi:64,name:'E4',on:'bottom treble'},
  {s:5,midi:59,name:'B3',between:'C4-A3'},
  {s:6,midi:56,name:'G#3',between:'A3 space below'},
  {s:7,midi:54,name:'F#3',between:'F3-A3'},
  {s:8,midi:52,name:'E3',between:'D3-F3'},
  {s:9,midi:50,name:'D3',on:'3rd bass'},
  {s:10,midi:47,name:'B2',on:'4th bass'}
];
for(var i=0;i<openStrs.length;i++){
  var os=openStrs[i],y=m2y(os.midi);
  var inRange=(y>=tT-3*ls && y<=bT+7*ls); // within staff + ledger range
  eq(inRange,true,'String '+os.s+' ('+os.name+') Y='+y+' in staff range');
}

// ── 11. Edge cases ──
describe('Edge cases');
var hzNull=cpd(null,[0,0,0],[0,0,0,0,0]);
var hzZero=cpd(0,[0,0,0],[0,0,0,0,0]);
for(var i=0;i<10;i++)
  near(hzNull[i],hzZero[i],0.01,'Str '+(i+1)+' null fret == fret 0');

// Partial pedal engagement (0.5 = half)
var hzHalf=cpd(null,[0.5,0,0],[0,0,0,0,0]);
var hzFull=cpd(null,[1,0,0],[0,0,0,0,0]);
for(var i=0;i<10;i++){
  var between=(hzHalf[i]>=hzOpen[i]-0.01 && hzHalf[i]<=hzFull[i]+0.01);
  eq(between,true,'Str '+(i+1)+' half P1 between open and full');}

// ── 12. Copedant JSON round-trip ──
describe('Copedant export/import round-trip');
var json=exportConfig();
var parsed=JSON.parse(json);
eq(parsed.copedant.name,'Geoff Derby E9','Name preserved');
eq(parsed.copedant.strings,10,'String count preserved');
eq(parsed.copedant.open_midi.length,10,'MIDI array length');
eq(parsed.copedant.pedal_names.length,3,'Pedal count');
eq(parsed.copedant.lever_names.length,5,'Lever count');
// Verify all pedal changes survived
eq(JSON.stringify(parsed.copedant.pedal_changes.P1),JSON.stringify([[4,2],[9,2]]),'P1 changes');
eq(JSON.stringify(parsed.copedant.pedal_changes.P2),JSON.stringify([[2,1],[5,1]]),'P2 changes');
eq(JSON.stringify(parsed.copedant.lever_changes.LKV),JSON.stringify([[4,-1],[9,-1]]),'LKV changes');

// ── 13. Coordinator: basic sustain ──
describe('Coordinator: basic sustain');
function mkPkt(bar,picks,peds,levs){
  return{timestamp_us:0,pedals:peds||[0,0,0],levers:levs||[0,0,0,0,0],
    bar_sens:bar!==null?sensResp(bar):[0,0,0,0],volume:0.8,
    picks:picks||new Array(10).fill(false)}}
coordReset();
var p1=mkPkt(10,[0,0,1,1,1,0,0,0,0,0]);
var f1=coordProcess(p1,1/60);
eq(f1.string_amp[2]>0.5,true,'String 3 attacked, amp='+f1.string_amp[2].toFixed(2));

// Sustain while active
for(var fr=0;fr<30;fr++)coordProcess(mkPkt(10,[0,0,1,1,1,0,0,0,0,0]),1/60);
eq(coord.amp[2]>0.3,true,'Active string sustains, amp='+coord.amp[2].toFixed(2));

// Release: fast decay
var ampBeforeRelease=coord.amp[2];
for(var fr=0;fr<10;fr++)coordProcess(mkPkt(10,[0,0,0,0,0,0,0,0,0,0]),1/60);
eq(coord.amp[2]<ampBeforeRelease*0.2,true,'Released string decays fast, amp='+coord.amp[2].toFixed(4));

// ── 14. Tab standard notation ──
describe('Tab standard pedal notation');
eq(TAB_PED[0],'A','P1 → A');
eq(TAB_PED[1],'B','P2 → B');
eq(TAB_PED[2],'C','P3 → C');

// ── Summary ──
console.log('\n════════════════════════════════');
console.log(' '+pass+' passed, '+fail+' failed');
console.log('════════════════════════════════');
process.exit(fail>0?1:0);
