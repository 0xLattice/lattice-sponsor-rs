#!/usr/bin/env python3
import copy,hashlib,json,pathlib,subprocess
root=pathlib.Path(__file__).resolve().parents[1];reference=root.parent/'lattice-sponsor';cache=pathlib.Path('/tmp/lattice-genesis-oracle');lattice=root.parent/'Lattice'
pin='24d8c4def3bd47c45b4cd10115762894e5185ea5'
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=reference,text=True).strip()==pin
assert not subprocess.check_output(['git','status','--porcelain','--untracked-files=no'],cwd=reference,text=True).strip()
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=lattice,text=True).strip()=='dda2d852429a27be6043599b63bde81530cd9514'
for name in ['MinerLoopLogic.swift','ProofOfWork.swift']:
 assert (cache/'Sources/LatticeMinerCore'/name).read_bytes()==(root.parent/'lattice-node/Sources/LatticeMinerCore'/name).read_bytes()
for p in (lattice/'Sources').rglob('*.swift'):assert p.read_bytes()==(cache/p.relative_to(lattice)).read_bytes()
assert hashlib.sha256((cache/'Package.resolved').read_bytes()).hexdigest()=='958f69bd3ec734a372e8cd94e3d09d593674a1025979a2abe2b45a6f08cfc9a6'
MAX=(1<<256)-1;hx=lambda n:hex(n);P=['Nexus'];Q=['Nexus','child']
def install(work='w',height=1,reward=8,target=MAX,expiry=100,now=0,path=P):return dict(op='install',workID=work,prefixHex='00',prices=[dict(path=path,target=hx(target),height=height,reward=reward)],submitTarget=hx(MAX),expiresAt=expiry,now=now)
def assign(client='alice',path=P,now=0,**kw):return dict(op='assign',client=client,path=path,now=now,**kw)
def credit(n=0,id='w.1',now=0,hash=0):return dict(op='credit',id=id,nonce=n,hash=hx(hash),now=now)
def block(n=0,id='w.1',now=0,hash=0):return dict(op='block',id=id,nonce=n,hash=hx(hash),now=now)
def balance(client='alice',path=P):return dict(op='balance',client=client,path=path)
def commit(fee=1,balance=10,now=0,until=50,client='alice',path=P):return dict(op='commit',client=client,fee=fee,balance=balance,now=now,holdUntil=until,path=path)
def trace(name,actions,share=MAX,ranges=8,capacity=100,lifetime=100):return dict(name=name,shareTarget=hx(share),rangeSize=ranges,maxAssignments=capacity,creditLifetime=lifetime,actions=actions)
arithmetic=[dict(shareTarget=hx(s),target=hx(t),reward=r,fee=f) for s in [1,2,(1<<64)-1,(1<<128)-1,(1<<192)-1,MAX//3,MAX] for t in [1,(1<<128)-1,MAX//3,MAX] for r in [0,1,1024,(1<<64)-1] for f in [0,1,(1<<64)-1]]
traces=[
trace('soldout',[install(),assign(),credit(),credit(1),credit(1),assign('bob'),block(1),block(1),balance()]),
trace('order',[assign(),credit(),install(),assign(),credit(9),credit(0,hash=MAX),credit(0),block(999),dict(op='assignment',id='missing')],share=MAX//2),
trace('commits',[install(),assign(),credit(),commit(9),commit(4,balance=3),commit(4),balance(),commit(4,balance=4),commit(4,balance=4,now=50),balance(),commit(0),install('later',height=2,now=101,expiry=200),balance()]),
trace('ranges',[install(),assign(),assign('bob'),assign('carol'),install('new',height=2),assign('dave')],ranges=(1<<63),capacity=100),
trace('capacity',[install(),assign(),assign('bob'),install('new',now=100,expiry=200),assign('bob'),dict(op='assignment',id='w.1')],capacity=1),
trace('reorg',[install('high',height=10,expiry=20),assign(),install('low',height=9,expiry=100,now=1),assign('bob'),credit(id='high.1',now=1),install('omitted',height=1,expiry=100,now=2,path=Q),assign('bob'),credit(id='high.1',now=20),install('low',height=9,expiry=100,now=20),assign('bob'),credit(id='high.1',now=20)]),
trace('sameheight',[install(),assign(),credit(),install('other'),assign('bob'),install('absent',height=5,path=Q,now=100,expiry=200),install('new',height=1,now=100,expiry=200),assign('bob'),install('higher',height=2,now=101,expiry=200),assign('bob'),credit(id='higher.2',now=101),balance('bob')]),
trace('credit-expiry',[install(expiry=1000),assign(),credit(now=10),balance(),install('b',height=2,now=109,expiry=1000),balance(),install('c',height=3,now=110,expiry=1000),balance()],lifetime=100),
trace('external-credit',[install(),assign(store='root'),credit(),balance(),install('b',height=2),assign(peer='peer'),credit(id='b.2'),balance()]),
trace('repeat-template',[install(expiry=10),assign(),install(expiry=100,now=5),assign('bob',now=5),credit(id='w.2',n=8,now=11),block(id='w.2',n=8,now=11),install(now=11,expiry=100),credit(id='w.2',n=8,now=11),assign('carol',now=11)]),
trace('partial',[install(reward=10,target=MAX//3),assign(),credit(),credit(1),credit(2),credit(3),credit(4),credit(4),balance(),dict(op='sharesFor',fee=1,path=P)]),
trace('missing-price',[install(reward=0),assign(),dict(op='sharesFor',fee=1,path=P),install(),assign(path=Q),dict(op='sharesFor',fee=1,path=Q)]),
]
base=dict(accountActions=[],actions=[],depositActions=[],receiptActions=[],withdrawalActions=[],signers=['client'],nonce=7,chainPath=P)
drafts=[]
def draft(name,body=None,fee=1,pay=0,sponsor='sponsor',path=P):drafts.append(dict(name=name,body=copy.deepcopy(body or base),fee=fee,pay=pay,sponsor=sponsor,path=path))
draft('plain');draft('pay',pay=7);draft('fee-zero',fee=0);draft('fee-overflow',fee=1<<63);draft('pay-overflow',pay=(1<<64)-1);draft('pay-int-overflow',fee=(1<<63)-1,pay=1);draft('wrong-chain',path=Q)
for name,patch in [('no-signers',dict(signers=[])),('two-signers',dict(signers=['client','other'])),('sponsor-signer',dict(signers=['sponsor'])),('named-account',dict(accountActions=[dict(owner='sponsor',delta=-1)])),('mentioned-key',dict(actions=[dict(key='prefixsponsorsuffix',oldValue=None,newValue='v')])),('mentioned-old',dict(actions=[dict(key='k',oldValue='sponsor',newValue=None)])),('mentioned-new',dict(actions=[dict(key='k',oldValue=None,newValue='sponsor')])),('creates-value',dict(accountActions=[dict(owner='client',delta=1)])),('balanced',dict(accountActions=[dict(owner='client',delta=-2),dict(owner='other',delta=2)])),('nonce-boundary',dict(nonce=(1<<64)-1))]:
 b=copy.deepcopy(base);b.update(patch);draft(name,b)
for field,action in [('depositActions',dict(nonce=0,demander='sponsor',amountDemanded=1,amountDeposited=1)),('receiptActions',dict(withdrawer='sponsor',nonce=0,demander='client',amountDemanded=1,directory='child')),('receiptActions',dict(withdrawer='client',nonce=0,demander='sponsor',amountDemanded=1,directory='child')),('withdrawalActions',dict(withdrawer='sponsor',nonce=0,demander='client',amountDemanded=1,amountWithdrawn=1)),('withdrawalActions',dict(withdrawer='client',nonce=0,demander='sponsor',amountDemanded=1,amountWithdrawn=1))]:
 b=copy.deepcopy(base);b[field]=[action];draft('named-'+field+'-'+action.get('withdrawer',''),b)
b=copy.deepcopy(base);b['accountActions']=[dict(owner='client',delta=-(1<<63))];draft('surplus-boundary',b)
serving=[dict(rate=r,value=hx(v),path=path) for r in [-1,0,1,3,(1<<63)-1] for v in [0,1,(1<<64)-1,1<<64,(1<<127),MAX] for path in [P,Q]]
peers=[dict(peer=p,store=s) for p in [None,'','a'*64,'0'*64,'A'*64,'g'*64,'a'*63,'é'*32] for s in [None,'root']]
payments=[dict(chain=c,txCID='tx',signer='alice',signature='signature',store=s,peer=p) for c in ['Nexus','Nexus/child','/Nexus//child/'] for s,p in [('r',None),(None,'a'*64),(None,None),('r','a'*64)]]
paymentTrace=[dict(op='claim',cid='tx'),dict(op='claim',cid='tx'),dict(op='release',cid='tx',credit=False),dict(op='claim',cid='tx'),dict(op='release',cid='tx',credit=True),dict(op='claim',cid='tx'),dict(op='credited'),dict(op='claim',cid='pending'),dict(op='restart'),dict(op='claim',cid='tx'),dict(op='claim',cid='pending'),dict(op='release',cid='other',credit=True),dict(op='credited')]
def upload(root='r',size=10,now=0,entries=None):
 v=dict(cid=root)
 if entries is None:v['bytes']=size
 else:v['entries']=entries
 return dict(op='upload',root=root,volumes=[v],now=now)
def op(kind,**kw):return dict(op=kind,**kw)
def st(name,actions,budget=100,capacity=100):return dict(name=name,actions=actions,budget=budget,capacity=capacity)
storage=[
 st('lifecycle',[upload(),op('status'),op('take'),op('credit',value=hx(1<<64)),op('status'),op('credit',value=hx(3<<64)),op('take'),op('take'),op('finish',pinned=False),op('take'),op('finish',pinned=True),op('status'),op('restart'),op('status'),upload()]),
 st('expiry',[upload(),op('check',now=9),op('check',now=10),upload('other',now=10),op('status'),upload(now=11),op('credit',value=hx(4<<64),now=11),upload('later',now=30),op('status'),op('check',now=30),op('take')]),
 st('budget',[upload(size=60),upload('other',size=41),upload('other',size=40),upload('third',size=1)]),
 st('pending-cap',[upload(size=30),upload('other',size=21),upload('other',size=20)],capacity=50),
 st('invalid',[op('upload',root='r',volumes=[]),upload(entries={'other':'AA=='}),upload(entries={'r':'!'}),upload(size=0),upload(size=-1),upload(size=(1<<20)+1025),upload(entries={'r':''}),op('status'),op('take')]),
 st('rates',[upload(entries={'r':'YWJj','x':'ZA=='}),op('credit',value=hx(1<<63)),op('status'),op('credit',value=hx(MAX),path=Q),op('status'),op('check',path=['Nexus','zero']),op('check',path=['missing'],root='missing')]),
 st('restart-pending',[upload(),op('credit',value=hx(4<<64)),op('restart'),op('status'),upload()]),
]
input=dict(arithmetic=arithmetic,traces=traces,drafts=drafts,serving=serving,peers=peers,payments=payments,paymentTrace=paymentTrace,storage=storage);inputpath=root/'tests/sponsor-input.json';inputpath.write_text(json.dumps(input,indent=2,sort_keys=True)+'\n')
# Compile the complete unchanged production ledger in the oracle executable.
target=cache/'Sources/SponsorOracle';target.mkdir(exist_ok=True);source=reference/'Sources/LatticeSponsorCore/SponsorLedger.swift';(target/'SponsorLedger.swift').write_bytes(source.read_bytes());(target/'Oracle.swift').write_bytes((root/'tests/sponsor-oracle.swift').read_bytes())
def extract(text,marker):
 start=text.index(marker);brace=text.index('{',start);depth=1;i=brace+1
 while depth:
  depth+=(text[i]=='{')-(text[i]=='}');i+=1
 return text[start:i]
extra='import Foundation\nimport UInt256\n'
extra_sources={}
for filename,markers in [('ServingCredit.swift',['public struct ServingCredit:']),('StorageCredit.swift',['public struct StoreVolume:','public struct StoreStatus:','public struct StorageLedger:']),('CoinPayment.swift',['public struct CoinPayment:','public struct PaymentLedger:'])]:
 original=reference/'Sources/LatticeSponsorCore'/filename;extra_sources[filename]=hashlib.sha256(original.read_bytes()).hexdigest()
 for marker in markers:extra+=extract(original.read_text(),marker)+'\n'
(target/'ExtraSources.swift').write_text(extra)
(target/'Extras.swift').write_bytes((root/'tests/sponsor-extras.swift').read_bytes())
p=cache/'Package.swift';s=p.read_text()
if 'name: "SponsorOracle"' not in s:s=s.replace('    targets: [\n','    targets: [\n        .executableTarget(name: "SponsorOracle", dependencies: ["Lattice", "LatticeMinerCore", .product(name: "UInt256", package: "UInt256")]),\n',1);p.write_text(s)
raw=subprocess.check_output(['swift','run','--package-path',str(cache),'--force-resolved-versions','SponsorOracle',str(inputpath)]);result=json.loads(raw);outpath=root/'tests/sponsor-swift.json';outpath.write_text(json.dumps(result,indent=2,sort_keys=True)+'\n')
actual=json.loads(subprocess.check_output(['cargo','run','--quiet','--example','sponsor_samples','--',str(inputpath)],cwd=root))
report=dict(revision=pin,arithmetic=len(arithmetic),traces=len(traces),steps=sum(len(t['actions']) for t in traces),drafts=len(drafts),extra_source_sha256=extra_sources,serving=len(serving),storage_traces=len(storage),storage_steps=sum(len(t['actions']) for t in storage),peer_checks=len(peers),payment_messages=len(payments),payment_steps=len(paymentTrace),source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),input_sha256=hashlib.sha256(inputpath.read_bytes()).hexdigest(),output_sha256=hashlib.sha256(outpath.read_bytes()).hexdigest());(root/'tests/sponsor-evidence.json').write_text(json.dumps(report,indent=2)+'\n')
if actual!=result:
 for category in result:
  if actual[category]!=result[category]:
   if isinstance(result[category],list):
    for i,(a,b) in enumerate(zip(actual[category],result[category])):
     if a!=b:print(category,i,'Rust',a,'Swift',b)
   else:
    for k in result[category]:
     if actual[category][k]!=result[category][k]:print(category,k,'Rust',actual[category][k],'Swift',result[category][k])
 raise SystemExit('mismatch')
print(report)
