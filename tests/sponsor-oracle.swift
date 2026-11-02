import Foundation
import Lattice
import LatticeMinerCore
import UInt256
func json<T:Encodable>(_ value:T)throws->Any {try JSONSerialization.jsonObject(with:JSONEncoder().encode(value),options:[.fragmentsAllowed])}
func uint(_ value:Any)throws->UInt256 {try JSONDecoder().decode(UInt256.self,from:JSONSerialization.data(withJSONObject:value,options:[.fragmentsAllowed]))}
func num(_ a:[String:Any],_ key:String)->UInt64{(a[key] as! NSNumber).uint64Value}
func signed(_ a:[String:Any],_ key:String)->Int64{(a[key] as! NSNumber).int64Value}
func outcome(_ action:()throws->Any)->Any{do{return ["ok":try action()]}catch SponsorRefusal.draft(let message){return ["error":"draft","message":message]}catch{return ["error":String(describing:error)]}}
func assignment(_ a:ShareAssignment)throws->[String:Any]{["id":a.id,"client":a.client,"chainPath":a.chainPath,"height":a.height,"workID":a.workID,"prefixHex":a.prefixHex,"shareTarget":try json(a.shareTarget),"shareValue":try json(a.shareValue),"blockValue":try json(a.blockValue),"startNonce":a.startNonce,"count":a.count,"expiresAt":a.expiresAt,"store":a.store as Any? ?? NSNull(),"peer":a.peer as Any? ?? NSNull()]}
@main struct Oracle {static func main() throws {let input=try JSONSerialization.jsonObject(with:Data(contentsOf:URL(fileURLWithPath:CommandLine.arguments[1]))) as! [String:Any];var arithmetic:[[String:Any]]=[]
for v in input["arithmetic"] as! [[String:Any]] {let share=try uint(v["shareTarget"]!);let price=ChainPrice(target:try uint(v["target"]!),height:1,reward:num(v,"reward"));var ledger=SponsorLedger(shareTarget:share,rangeSize:1,maxAssignments:1,creditLifetime:100);ledger.install(SponsorTemplate(workID:"w",prefixHex:"00",prices:[["Nexus"]:price],submitTarget:price.target,expiresAt:100),now:0);arithmetic.append(["value":try json(SponsorLedger.shareValue(shareTarget:share,at:price)),"shares":try json(ledger.sharesFor(fee:num(v,"fee"),on:["Nexus"]))])}
var traces:[String:[Any]]=[:]
for trace in input["traces"] as! [[String:Any]] {var ledger=SponsorLedger(shareTarget:try uint(trace["shareTarget"]!),rangeSize:num(trace,"rangeSize"),maxAssignments:Int(num(trace,"maxAssignments")),creditLifetime:signed(trace,"creditLifetime"));var results:[Any]=[]
for a in trace["actions"] as! [[String:Any]] {let now=(a["now"] as? NSNumber)?.int64Value ?? 0;let result:Any
switch a["op"] as! String {
case "install":var prices:[[String]:ChainPrice]=[:];for p in a["prices"] as! [[String:Any]] {prices[p["path"] as! [String]]=ChainPrice(target:try uint(p["target"]!),height:num(p,"height"),reward:num(p,"reward"))};ledger.install(SponsorTemplate(workID:a["workID"] as! String,prefixHex:a["prefixHex"] as! String,prices:prices,submitTarget:try uint(a["submitTarget"]!),expiresAt:signed(a,"expiresAt")),now:now);result=NSNull()
case "assign":result=outcome{try assignment(ledger.assign(client:a["client"] as! String,on:a["path"] as! [String],store:a["store"] as? String,peer:a["peer"] as? String,now:now))}
case "credit":result=outcome{try json(ledger.credit(assignment:a["id"] as! String,nonce:num(a,"nonce"),hash:try uint(a["hash"]!),now:now))}
case "block":result=try json(ledger.block(assignment:a["id"] as! String,nonce:num(a,"nonce"),hash:try uint(a["hash"]!),now:now))
case "commit":result=outcome{try ledger.commit(client:a["client"] as! String,fee:num(a,"fee"),on:a["path"] as! [String],balance:num(a,"balance"),holdUntil:signed(a,"holdUntil"),now:now);return NSNull()}
case "balance":result=try json(ledger.credit(of:a["client"] as! String,on:a["path"] as! [String]))
case "sharesFor":result=try json(ledger.sharesFor(fee:num(a,"fee"),on:a["path"] as! [String]))
case "assignment":result=try ledger.assignment(a["id"] as! String).map{try assignment($0)} ?? NSNull()
default:fatalError("unknown action")};results.append(result)};traces[trace["name"] as! String]=results}
var drafts:[Any]=[];for d in input["drafts"] as! [[String:Any]] {let body=try JSONDecoder().decode(TransactionBody.self,from:JSONSerialization.data(withJSONObject:d["body"]!));drafts.append(outcome{try json(sponsoredBody(draft:body,sponsor:d["sponsor"] as! String,fee:num(d,"fee"),pay:num(d,"pay"),chainPath:d["path"] as! [String]))})}
let output:[String:Any]=["arithmetic":arithmetic,"traces":traces,"drafts":drafts,"extras":try extras(input)];print(String(decoding:try JSONSerialization.data(withJSONObject:output,options:[.sortedKeys]),as:UTF8.self))}}
