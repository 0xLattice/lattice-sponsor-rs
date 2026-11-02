#!/usr/bin/env python3
"""Exercise real Rust node + sponsor processes on a disposable localhost chain.

Build both binaries first. Does not contact any configured/public network.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import queue
import shutil
import subprocess
import tempfile
import threading
import time
import urllib.request
import urllib.error

ROOT = Path(__file__).resolve().parents[1]


def request(base, route, body=None, cookie=None):
    headers = {'Content-Type': 'application/json'}
    if cookie is not None:
        headers['Authorization'] = 'Basic ' + base64.b64encode(cookie.encode()).decode()
    req = urllib.request.Request(base + route, data=None if body is None else json.dumps(body).encode(), headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=5) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)


def spawn(command, stream, parse, logs):
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    ready = queue.Queue()
    def drain(pipe, selected):
        for line in pipe:
            logs.append(line.rstrip())
            if selected:
                try:
                    value = parse(line)
                    if value is not None:
                        ready.put(value)
                except Exception as error:
                    ready.put(error)
        if selected:
            ready.put(RuntimeError('process exited before ready'))
    threading.Thread(target=drain, args=(child.stdout, stream == 'stdout'), daemon=True).start()
    threading.Thread(target=drain, args=(child.stderr, stream == 'stderr'), daemon=True).start()
    try:
        value = ready.get(timeout=15)
        if isinstance(value, Exception):
            raise value
        return child, value
    except Exception:
        child.kill()
        child.wait()
        raise RuntimeError('process did not become ready: ' + '\n'.join(logs[-8:]))


def stop(child):
    child.terminate()
    try:
        code = child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()
        raise RuntimeError('process did not shut down')
    if code != 0:
        raise RuntimeError(f'process exited with {code}')


def refreshed_work(base, parameters, previous_prefix=None):
    if previous_prefix is None:
        code, previous = request(base, '/work', parameters)
        assert code == 200, previous
        previous_prefix = previous['prefixHex']
    deadline = time.monotonic() + 15
    while True:
        code, current = request(base, '/work', parameters)
        if code == 200 and current['prefixHex'] != previous_prefix:
            return current
        assert code == 200 or current.get('error') == 'soldOut', current
        assert time.monotonic() < deadline, 'sponsor did not refresh template'
        time.sleep(0.1)


def submit_share(base, work):
    nonce = int(work['startNonce'])
    digest = hashlib.sha256(bytes.fromhex(work['prefixHex']) + nonce.to_bytes(8, 'big')).digest()
    assert int.from_bytes(digest, 'big') <= int(work['shareTarget'], 16)
    status, result = request(base, '/shares', {'assignment': work['assignment'], 'nonce': str(nonce)})
    assert status == 200, result
    return result


def mine_root(base, cookie, recipients=None):
    status, template = request(base, '/mining/templates', {'recipients': recipients or []}, cookie)
    assert status == 200, template
    status, mined = request(base, '/mining/work', {'workID': template['workID'], 'nonce': 0}, cookie)
    assert status == 200, mined
    status, state = request(base, '/status', cookie=cookie)
    assert status == 200, state
    return state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/node-integration.json')
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = {'passed': False, 'scope': 'disposable localhost node and sponsor'}
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    subprocess.run(['cargo', 'build', '--quiet', '--locked', '--example', 'integration_signer'], cwd=ROOT, check=True)
    signer = ROOT / 'target/debug/examples/integration_signer'
    node_bin = ROOT.parent / 'lattice-node-rs/target/debug/lattice-node'
    sponsor_bin = ROOT / 'target/debug/lattice-sponsor'
    fixture = ROOT.parent / 'lattice-rs/crates/lattice-validation/tests/genesis-swift.json'
    genesis = json.loads(fixture.read_text())[0]
    processes, node_logs, sponsor_logs = [], [], []
    try:
        with tempfile.TemporaryDirectory(prefix='lattice-platform-integration-') as tmp:
            tmp = Path(tmp)
            # Execute immutable per-run copies, including during restart, while
            # other agents may rebuild the shared target directories.
            binaries = tmp / 'bin'
            binaries.mkdir()
            node_bin = Path(shutil.copy2(node_bin, binaries / node_bin.name))
            sponsor_bin = Path(shutil.copy2(sponsor_bin, binaries / sponsor_bin.name))
            signer = Path(shutil.copy2(signer, binaries / signer.name))
            binary_hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in (node_bin, sponsor_bin, signer)}
            with (tmp / 'bootstrap.json').open('w') as output:
                subprocess.run(['cargo', 'run', '--quiet', '--locked', '-p', 'lattice-validation', '--example', 'genesis_volumes', '--', str(fixture)], cwd=ROOT.parent / 'lattice-rs', stdout=output, check=True)
            config = {'directory': 'data', 'identityKey': 'identity.key', 'genesisCid': genesis['cid'],
                      'bootstrapFiles': ['bootstrap.json'], 'minRelayFee': 0, 'listen': '127.0.0.1:0', 'rpc': '127.0.0.1:0'}
            config_path = tmp / 'node.json'
            config_path.write_text(json.dumps(config))
            node, ready = spawn([str(node_bin), '--config', str(config_path)], 'stdout', lambda line: json.loads(line) if line.startswith('{') else None, node_logs)
            processes.append(node)
            cookie_path = tmp / 'data/.cookie'
            cookie = cookie_path.read_text()
            node_url = 'http://' + ready['rpc']
            status, before = request(node_url, '/status', cookie=cookie)
            assert status == 200 and before['height'] == '0', before
            key = tmp / 'sponsor-key.json'
            key.write_text(json.dumps({'privateKey': '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60',
                                      'publicKey': 'ed01d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'}))
            key.chmod(0o600)
            sponsor, address = spawn([str(sponsor_bin), '--node', node_url, '--cookie-file', str(cookie_path), '--key', str(key),
                                      '--share-work', '1', '--fee', '1', '--sell-storage', '--storage-rate', 'Nexus=1',
                                      '--storage-file', str(tmp / 'stored.json'), '--serving-rate', 'Nexus=1',
                                      '--pay-to', 'bafyreiggauicvrzpau2vtx6kozmbgb3zhfvsntd4g6hjrjbxcx3gmfi4ym', '--coin-depth', '1', '--port', '0'], 'stderr',
                                     lambda line: line.strip().split(' listening on ', 1)[1] if ' listening on ' in line else None, sponsor_logs)
            processes.append(sponsor)
            sponsor_url = 'http://' + address
            deadline = time.monotonic() + 20
            while True:
                status, work = request(sponsor_url, '/work', {'chain': 'Nexus'})
                if status == 200:
                    break
                assert time.monotonic() < deadline, (status, work, sponsor_logs[-8:])
                time.sleep(0.1)
            nonce = int(work['startNonce'])
            digest = hashlib.sha256(bytes.fromhex(work['prefixHex']) + nonce.to_bytes(8, 'big')).digest()
            assert int.from_bytes(digest, 'big') <= int(work['shareTarget'], 16)
            status, share = request(sponsor_url, '/shares', {'assignment': work['assignment'], 'nonce': str(nonce)})
            assert status == 200, (status, share, sponsor_logs[-8:])
            assert int(share['credit']) > 0, share
            status, after = request(node_url, '/status', cookie=cookie)
            assert status == 200 and after['height'] == '1', (after, sponsor_logs[-8:])
            assert after['tipCID'] != before['tipCID']
            status, duplicate = request(sponsor_url, '/shares', {'assignment': work['assignment'], 'nonce': str(nonce)})
            assert status == 400 and duplicate.get('error') == 'duplicateShare', duplicate
            duplicate_status = status
            draft = json.loads(subprocess.check_output([str(signer)], text=True))
            deadline = time.monotonic() + 15
            while True:
                status, sponsored = request(sponsor_url, '/sponsor', {'client': work['client'], 'transaction': {'body': draft, 'signatures': {}}})
                if status == 200:
                    break
                assert time.monotonic() < deadline, (status, sponsored)
                time.sleep(0.1)
            (tmp / 'sponsored.json').write_text(json.dumps(sponsored))
            signed = json.loads(subprocess.check_output([str(signer), str(tmp / 'sponsored.json')], text=True))
            status, submitted = request(node_url, '/transactions', signed, cookie)
            assert 200 <= status < 300, (status, submitted)
            status, template = request(node_url, '/mining/templates', {'recipients': []}, cookie)
            assert status == 200, template
            status, mined = request(node_url, '/mining/work', {'workID': template['workID'], 'nonce': 0}, cookie)
            assert status == 200, mined
            status, after = request(node_url, '/status', cookie=cookie)
            assert status == 200 and after['height'] == '2', after
            status, inclusion = request(node_url, '/api/transaction/' + submitted['transactionCID'], cookie=cookie)
            assert status == 200 and inclusion.get('blockHash') == after['tipCID'], inclusion
            inclusion_block = inclusion['blockHash']
            volume_bytes = b'\x00'
            volume_cid = 'b' + base64.b32encode(bytes([1, 0x71, 0x12, 0x20]) + hashlib.sha256(volume_bytes).digest()).decode().lower().rstrip('=')
            status, uploaded = request(sponsor_url, '/store', {'root': volume_cid, 'volumes': [{'cid': volume_cid, 'entries': {volume_cid: base64.b64encode(volume_bytes).decode()}}]})
            assert status == 200 and uploaded['stored'] is False, uploaded
            store_work = refreshed_work(sponsor_url, {'chain': 'Nexus', 'store': volume_cid})
            submit_share(sponsor_url, store_work)
            deadline = time.monotonic() + 10
            while True:
                status, stored = request(sponsor_url, '/store/' + volume_cid)
                if status == 200 and stored['stored']:
                    break
                assert time.monotonic() < deadline, stored
                time.sleep(0.1)
            status, pins = request(node_url, '/pins', cookie=cookie)
            assert status == 200 and volume_cid in pins['pins'], pins
            peer = '11' * 32
            status, prior_standing = request(node_url, '/serving-credit/' + peer, cookie=cookie)
            assert status == 200, prior_standing
            serve_work = refreshed_work(sponsor_url, {'chain': 'Nexus', 'peer': peer}, store_work['prefixHex'])
            serving = submit_share(sponsor_url, serve_work)
            status, standing = request(node_url, '/serving-credit/' + peer, cookie=cookie)
            assert status == 200 and standing['standing'] - prior_standing['standing'] == int(serving['serving']), standing
            assert int(serving['serving']) > 0
            status, after = request(node_url, '/status', cookie=cookie)
            assert status == 200 and after['height'] == '4', after
            # Fund the client on this fixture chain, then pay the sponsor.
            after = mine_root(node_url, cookie, [{'chainPath': ['Nexus'], 'address': draft['signers'][0]}])
            assert after['height'] == '5', after
            status, terms = request(sponsor_url, '/terms')
            assert status == 200 and terms['depth'] == '1', terms
            payment_body = dict(draft, nonce='1', accountActions=[
                {'owner': draft['signers'][0], 'delta': '-3'},
                {'owner': terms['payTo'], 'delta': '3'},
            ])
            (tmp / 'payment-tx.json').write_text(json.dumps({'transaction': {'body': payment_body, 'signatures': {}}}))
            payment_tx = json.loads(subprocess.check_output([str(signer), str(tmp / 'payment-tx.json')], text=True))
            status, paid_tx = request(node_url, '/transactions', payment_tx, cookie)
            assert status == 200, paid_tx
            after = mine_root(node_url, cookie)
            assert after['height'] == '6', after
            payment = {'chain': 'Nexus', 'txCID': paid_tx['transactionCID'], 'signer': draft['signers'][0], 'signature': '', 'peer': peer}
            (tmp / 'payment.json').write_text(json.dumps(payment))
            payment = json.loads(subprocess.check_output([str(signer), '--payment', str(tmp / 'payment.json')], text=True))
            status, shallow = request(sponsor_url, '/pay', payment)
            assert status == 425 and shallow.get('error') == 'notDeepEnough', shallow
            after = mine_root(node_url, cookie)
            assert after['height'] == '7', after
            status, forged = request(sponsor_url, '/pay', dict(payment, signature='00' * 64))
            assert status == 400 and forged.get('error') == 'invalidSignature', forged
            status, unchanged = request(node_url, '/serving-credit/' + peer, cookie=cookie)
            assert status == 200 and unchanged == standing, unchanged
            status, paid = request(sponsor_url, '/pay', payment)
            assert status == 200 and paid['bytes'] == '3', paid
            status, paid_standing = request(node_url, '/serving-credit/' + peer, cookie=cookie)
            assert status == 200 and paid_standing['standing'] - standing['standing'] == 3, paid_standing
            standing = paid_standing
            status, replay = request(sponsor_url, '/pay', payment)
            assert status == 409 and replay.get('error') == 'alreadyCredited', replay
            sponsor_command = sponsor.args.copy()
            stop(sponsor)
            processes.remove(sponsor)
            stop(node)
            processes.remove(node)
            config['bootstrapFiles'] = []
            config_path.write_text(json.dumps(config))
            node, restarted = spawn([str(node_bin), '--config', str(config_path)], 'stdout', lambda line: json.loads(line) if line.startswith('{') else None, node_logs)
            processes.append(node)
            status, recovered = request('http://' + restarted['rpc'], '/status', cookie=cookie_path.read_text())
            assert status == 200 and recovered['tipCID'] == after['tipCID'] and recovered['height'] == '7', recovered
            status, inclusion = request('http://' + restarted['rpc'], '/api/transaction/' + submitted['transactionCID'], cookie=cookie_path.read_text())
            assert status == 200 and inclusion.get('blockHash') == inclusion_block, inclusion
            status, recovered_pins = request('http://' + restarted['rpc'], '/pins', cookie=cookie_path.read_text())
            assert status == 200 and volume_cid in recovered_pins['pins'], recovered_pins
            status, recovered_standing = request('http://' + restarted['rpc'], '/serving-credit/' + peer, cookie=cookie_path.read_text())
            assert status == 200 and recovered_standing == standing, recovered_standing
            sponsor_command[sponsor_command.index('--node') + 1] = 'http://' + restarted['rpc']
            sponsor, address = spawn(sponsor_command, 'stderr', lambda line: line.strip().split(' listening on ', 1)[1] if ' listening on ' in line else None, sponsor_logs)
            processes.append(sponsor)
            status, recovered_store = request('http://' + address, '/store/' + volume_cid)
            assert status == 200 and recovered_store['stored'], recovered_store
            status, replay = request('http://' + address, '/pay', payment)
            assert status == 409 and replay.get('error') == 'alreadyCredited', replay
            stop(sponsor)
            processes.remove(sponsor)
            stop(node)
            processes.remove(node)
            report.update(passed=True, height=recovered['height'], tip_cid=recovered['tipCID'],
                          duplicate_status=duplicate_status, credited=share['credit'], sponsored_fee=sponsored['fee'], submission=submitted, inclusion_block=inclusion['blockHash'], stored_cid=volume_cid, serving_bytes=serving['serving'], coin_payment_bytes=paid['bytes'], coin_payment_cid=paid_tx['transactionCID'],
                          genesis_fixture_sha256=hashlib.sha256(fixture.read_bytes()).hexdigest(),
                          binaries=binary_hashes)
    finally:
        for child in processes:
            child.kill()
            child.wait()
        args.output.with_suffix('.log').write_text('\n'.join(node_logs + sponsor_logs) + '\n')
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
