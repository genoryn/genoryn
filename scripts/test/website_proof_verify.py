#!/usr/bin/env python3
"""genoryn.io/verify end to end on a kept devnet lab (devnet.py --keep).

Starts lab node `a` again, the website server against it with a valueless
verifier wallet, and checks the HTTP API with the lab's real payment proofs:
verified payment, mismatches, forgery, receiving-side proof, and a fresh
sender challenge answered by the paying wallet (a replayed answer fails)."""
import argparse, base64, json, os, pathlib, subprocess, time, urllib.error, urllib.request

PASSWORD = "GENORYN-local-test-only-7!"


def browser_flow(args, proof_text, recipient, wallet):
    """The page as a person uses it: paste, verify, challenge, paste the answer."""
    from playwright.sync_api import sync_playwright
    out = args.screenshots.resolve()
    out.mkdir(parents=True, exist_ok=True)
    code = "GNRPROOF1-" + base64.urlsafe_b64encode(json.dumps(json.loads(proof_text), separators=(",", ":")).encode()).decode().rstrip("=")
    with sync_playwright() as p:
        browser = p.chromium.launch(channel="msedge")
        page = browser.new_page(viewport={"width": 1400, "height": 1000})
        page.goto(f"http://127.0.0.1:{args.port}/verify")
        page.screenshot(path=str(out / "verify-empty.png"))
        page.fill("#proof", code[:70] + "\n" + code[70:])
        page.fill("#to", recipient)
        page.fill("#amount", "1")
        page.get_by_role("button", name="Verify payment").click()
        page.get_by_text("Payment verified").wait_for(timeout=120_000)
        page.get_by_text("The amount is exactly the amount you entered.").wait_for()
        page.locator("form.calc").screenshot(path=str(out / "verify-result.png"))
        page.get_by_role("button", name="Request sender verification").click()
        challenge = page.locator(".code pre").first
        challenge.wait_for(timeout=60_000)
        answer = wallet("answer_payment_challenge", challenge=challenge.inner_text().strip())["response"]
        page.fill("#response", answer)
        page.get_by_role("button", name="Verify sender response").click()
        page.get_by_text("Sender verified").wait_for(timeout=120_000)
        page.locator(".card", has_text="Who sent it?").screenshot(path=str(out / "verify-sender.png"))
        page.screenshot(path=str(out / "verify-full.png"), full_page=True)
        browser.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lab", type=pathlib.Path, required=True)
    parser.add_argument("--bin-dir", type=pathlib.Path, default=pathlib.Path("target/debug"))
    parser.add_argument("--website", type=pathlib.Path, default=pathlib.Path("website"))
    parser.add_argument("--port", type=int, default=3123)
    parser.add_argument("--screenshots", type=pathlib.Path, help="also drive the page in Edge (Playwright) and save screenshots here")
    args = parser.parse_args()
    lab, suffix = args.lab.resolve(), ".exe" if os.name == "nt" else ""
    cli = str((args.bin_dir / f"genoryn-wallet-cli{suffix}").resolve())
    ports = json.loads((lab / "lab-public.json").read_text())["ports"]["a"]
    rpc = f"http://127.0.0.1:{ports[1]}"
    work = lab / "website-verify"
    work.mkdir(exist_ok=True)
    (work / "verifier.pass").write_text("verifier-test-only-31!")
    checks, processes = [], []

    def record(name, **extra):
        checks.append({"check": name, "passed": True, **extra})
        print(json.dumps(checks[-1]), flush=True)

    def wallet(op, **fields):
        request = dict(op=op, network="devnet", wallet_path=str(lab / "alice.wallet"), password=PASSWORD, rpc_url=rpc, token_file=str(lab / "a" / "admin.token"), **fields)
        out = json.loads(subprocess.run([cli, "--json-stdio"], input=json.dumps(request), capture_output=True, text=True, timeout=300).stdout.strip().splitlines()[-1])
        assert out["ok"], out
        return out["result"]

    base = f"http://127.0.0.1:{args.port}/api/v1"

    def post(path, body):
        request = urllib.request.Request(base + path, data=json.dumps(body).encode(), headers={"content-type": "application/json"}, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    try:
        node_log = open(work / "node.log", "ab")
        processes.append(subprocess.Popen([str((args.bin_dir / f"genorynd{suffix}").resolve()), "--network", "devnet", "--data-dir", str(lab / "a"), "--p2p-listen", f"127.0.0.1:{ports[0]}", "--rpc-listen", f"127.0.0.1:{ports[1]}"], stdout=node_log, stderr=node_log))
        for _ in range(100):
            try:
                urllib.request.urlopen(rpc + "/v1/info", timeout=1)
                break
            except OSError:
                time.sleep(0.2)
        created = subprocess.run([cli, "--json-stdio"], input=json.dumps(dict(op="create", network="devnet", wallet_path=str(work / "verifier.wallet"), password="verifier-test-only-31!")), capture_output=True, text=True, timeout=120)
        assert (work / "verifier.wallet").exists(), created.stdout[-300:]
        env = dict(os.environ, PORT=str(args.port), HOST="127.0.0.1", GENORYN_NETWORK="devnet", DB_PATH=str(work / "index.sqlite"), ADMIN_RPC_URL=rpc, PUBLIC_RPC_URL=rpc, PEER_SOURCES=f"lab={rpc}",
                   WEB_DIST=str((args.website / "apps/web/dist/client").resolve()), VERIFIER_CLI=cli, VERIFIER_WALLET=str(work / "verifier.wallet"), VERIFIER_PASSWORD_FILE=str(work / "verifier.pass"), LOG_LEVEL="error")
        server_log = open(work / "server.log", "ab")
        processes.append(subprocess.Popen(["node", str((args.website / "apps/server/dist/index.js").resolve())], env=env, stdout=server_log, stderr=server_log))
        for _ in range(100):
            try:
                with urllib.request.urlopen(base + "/proof/status", timeout=1) as response:
                    status = json.load(response)
                break
            except OSError:
                time.sleep(0.2)
        assert status == {"enabled": True, "network": "devnet"}, status
        record("verifier_enabled")

        sender_text = (lab / "proof-sender.json").read_text()
        received_text = (lab / "proof-received.json").read_text()
        proof = json.loads(sender_text)
        code, result = post("/proof/verify", {"proof": sender_text, "expected_recipient": proof["recipient"], "expected_amount": "1"})
        assert code == 200 and result["outcome"] == "payment_verified", (code, result)
        assert result["amount_atomic"] == "1000000000000" and result["recipient_matches"] is True and result["amount_matches"] is True, result
        assert result["created_by_sender"] is True and result["sender_evidence"] == "fresh_authentication_available" and result["memo"] == "private devnet invoice", result
        assert "belongs_to_wallet" not in result, "the verifier wallet's own view is never returned"
        record("sender_proof_verified_with_exact_amount_address_and_memo", block=result["block_height"], confirmations=result["confirmations"])

        code, result = post("/proof/verify", {"proof": sender_text, "expected_recipient": json.loads((lab / "lab-public.json").read_text())["alice_address"], "expected_amount": "1.000000000001"})
        assert code == 200 and result["recipient_matches"] is False and result["amount_matches"] is False, result
        record("wrong_address_and_amount_reported_as_mismatch")

        forged = sender_text.replace('"amount": 1000000000000', '"amount": 2000000000000')
        assert forged != sender_text
        code, result = post("/proof/verify", {"proof": forged})
        assert code == 422 and "error" in result, (code, result)
        record("forged_amount_rejected", error=result["error"])

        code, result = post("/proof/verify", {"proof": received_text})
        assert code == 200 and result["outcome"] == "payment_verified" and result["created_by_sender"] is False and result["sender_evidence"] == "none", result
        record("receiving_side_proof_verified_without_sender_claim")

        missing = json.dumps({**proof, "txid": "00" * 32})
        code, result = post("/proof/verify", {"proof": missing})
        assert code == 200 and result["outcome"] == "not_on_chain", (code, result)
        record("unknown_payment_is_not_on_chain")

        code, issued = post("/proof/challenge", {"proof": sender_text})
        assert code == 200 and issued["challenge"].startswith("GNRCHAL1-") and issued["expires"] > time.time(), (code, issued)
        answer = wallet("answer_payment_challenge", challenge=issued["challenge"])["response"]
        code, result = post("/proof/sender", {"proof": sender_text, "challenge": issued["challenge"], "response": answer[:60] + "\n" + answer[60:]})
        assert code == 200 and result["outcome"] == "sender_verified", (code, result)
        record("paying_wallet_answers_fresh_challenge")

        code, second = post("/proof/challenge", {"proof": sender_text})
        code, result = post("/proof/sender", {"proof": sender_text, "challenge": second["challenge"], "response": answer})
        assert code == 422, (code, result)
        record("replayed_answer_fails_a_new_challenge", error=result["error"])
        if args.screenshots:
            browser_flow(args, sender_text, proof["recipient"], wallet)
            record("page_verifies_and_challenges_in_a_real_browser")
        print(json.dumps({"passed": True, "checks": len(checks)}))
    finally:
        for process in reversed(processes):
            process.terminate()
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                process.kill()


if __name__ == "__main__":
    main()
