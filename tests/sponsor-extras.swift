import Foundation
import UInt256
func storeStatus(_ s:StoreStatus)->[String:Any]{["root":s.root,"bytes":s.bytes,"needed":s.needed,"credit":s.credit,"stored":s.stored]}
func extras(_ input:[String:Any])throws->[String:Any]{var serving:[Any]=[];for r in input["serving"] as! [[String:Any]] {let x=ServingCredit(rates:[["Nexus"]:Int(signed(r,"rate"))]);serving.append(outcome{try x.bytes(for:try uint(r["value"]!),on:r["path"] as! [String])})}
let peers=(input["peers"] as! [[String:Any]]).map{r in outcome{try ServingCredit.check(peer:r["peer"] as? String,store:r["store"] as? String);return NSNull()}}
let payments=try (input["payments"] as! [[String:Any]]).map{r->[String:Any] in let p=try JSONDecoder().decode(CoinPayment.self,from:JSONSerialization.data(withJSONObject:r));return ["message":p.message,"path":p.chainPath]}
let directory=FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString);try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true);defer{try? FileManager.default.removeItem(at:directory)}
let file=directory.appendingPathComponent("payments.json");var ledger=try PaymentLedger(file:file);var paymentTrace:[Any]=[]
for a in input["paymentTrace"] as! [[String:Any]] {let cid=a["cid"] as? String ?? "";let out:Any;switch a["op"] as! String{case "claim":out=outcome{try ledger.claim(cid);return NSNull()};case "release":try ledger.release(cid,credit:a["credit"] as! Bool);out=NSNull();case "restart":ledger=try PaymentLedger(file:file);out=NSNull();case "credited":out=ledger.credited.sorted();default:fatalError()};paymentTrace.append(out)}
var storage:[String:[Any]]=[:]
for t in input["storage"] as! [[String:Any]]{let file=directory.appendingPathComponent((t["name"] as! String)+".json");let rates=[["Nexus"]:3,["Nexus","child"]:7,["Nexus","zero"]:0];var ledger=try StorageLedger(budget:Int(signed(t,"budget")),rates:rates,file:file,pendingCap:Int(signed(t,"capacity")),lifetime:10);var outputs:[Any]=[];for a in t["actions"] as! [[String:Any]]{let root=a["root"] as? String ?? "r";let now=(a["now"] as? NSNumber)?.int64Value ?? 0;let path=a["path"] as? [String] ?? ["Nexus"];let out:Any;switch a["op"] as! String {
case "upload":out=outcome{let volumes=try JSONDecoder().decode([StoreVolume].self,from:JSONSerialization.data(withJSONObject:a["volumes"]!));return storeStatus(try ledger.upload(root:root,volumes:volumes,now:now))}
case "credit":out=outcome{try ledger.credit(root:root,value:try uint(a["value"]!),on:path,now:now);return NSNull()}
case "check":out=outcome{try ledger.check(root:root,on:path,now:now);return NSNull()}
case "status":out=ledger.status(of:root).map{storeStatus($0)} ?? NSNull()
case "take":if let u=ledger.takePaid(root){out=["bytes":u.bytes,"volumes":try json(u.volumes)]}else{out=NSNull()}
case "finish":try ledger.finish(root,pinned:a["pinned"] as! Bool);out=NSNull()
case "restart":ledger=try StorageLedger(budget:Int(signed(t,"budget")),rates:rates,file:file,pendingCap:Int(signed(t,"capacity")),lifetime:10);out=NSNull()
default:fatalError()};outputs.append(out)};storage[t["name"] as! String]=outputs}
return ["serving":serving,"peers":peers,"payments":payments,"paymentTrace":paymentTrace,"storage":storage]}
