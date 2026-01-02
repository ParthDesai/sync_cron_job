/**
 * Minimal EVM/Base bridge for sync_cron (no Rust EVM deps required).
 *
 * Usage:
 *   node scripts/evm_bridge.js <command>
 * Input:  JSON on stdin
 * Output: JSON on stdout
 */
const fs = require("fs");

let ethers;
try {
  // eslint-disable-next-line global-require
  ({ ethers } = require("ethers"));
} catch (e) {
  process.stderr.write(
    JSON.stringify({
      error:
        "Missing node dependency 'ethers'. From /Users/parth/projects/sync_cron run: npm install"
    })
  );
  process.exit(1);
}

const SYNC_ABI = [
  "function createAgent()",
  "function allowAgent(address agent)",
  // UPDATED submitData signature (contract stores timestamp internally from block.timestamp)
  "function submitData(string dataLink,string primaryCategory,string secondaryCategory,string domain,string dataType,string dataFormat,uint256 fileSizeInKB)",
  "function rateData(string dataLink,bool isSeedDeleted,bool isValid,bool hasRating,uint8 rating,bool hasSyntheticDataLink,string syntheticDataLink,bool sendTokensImmediately)",
  "function claimCredits()",
  "function agentConfigs(address) view returns (bool exists,bool isEnabled)",
  "function accumulatedCredits(address) view returns (uint256)",
  "function getSubmissionKey(string dataLink) pure returns (bytes32)"
];

function readStdinJson() {
  const raw = fs.readFileSync(0, "utf8");
  const trimmed = raw.trim();
  if (!trimmed) return {};
  return JSON.parse(trimmed);
}

function out(obj) {
  process.stdout.write(JSON.stringify(obj));
}

function fail(message, extra) {
  const err = { error: String(message) };
  if (extra) err.extra = extra;
  process.stderr.write(JSON.stringify(err));
  process.exitCode = 1;
}

async function provider(rpcUrl) {
  if (!rpcUrl || typeof rpcUrl !== "string") throw new Error("rpcUrl is required");
  return new ethers.JsonRpcProvider(rpcUrl);
}

async function contract(rpcUrl, proxyAddress, signerOrProvider) {
  if (!proxyAddress || typeof proxyAddress !== "string") throw new Error("proxyAddress is required");
  const base = signerOrProvider || (await provider(rpcUrl));
  return new ethers.Contract(proxyAddress, SYNC_ABI, base);
}

async function main() {
  const cmd = process.argv[2];
  if (!cmd) {
    fail("missing command");
    return;
  }

  try {
    const input = readStdinJson();

    if (cmd === "create_random_wallet") {
      const w = ethers.Wallet.createRandom();
      out({ address: await w.getAddress(), privateKey: w.privateKey });
      return;
    }

    if (cmd === "derive_address") {
      const pk = input.privateKey;
      const w = new ethers.Wallet(pk);
      out({ address: await w.getAddress() });
      return;
    }

    if (cmd === "get_balance") {
      const p = await provider(input.rpcUrl);
      const bal = await p.getBalance(input.address);
      out({ wei: bal.toString() });
      return;
    }

    if (cmd === "send_eth") {
      const p = await provider(input.rpcUrl);
      const w = new ethers.Wallet(input.fromPrivateKey, p);
      const tx = await w.sendTransaction({ to: input.to, value: BigInt(input.wei) });
      const rec = await tx.wait();
      out({ txHash: tx.hash, blockNumber: rec ? Number(rec.blockNumber) : null });
      return;
    }

    if (cmd === "agent_config") {
      const p = await provider(input.rpcUrl);
      const c = await contract(input.rpcUrl, input.proxyAddress, p);
      const cfg = await c.agentConfigs(input.address);
      out({ exists: Boolean(cfg.exists), isEnabled: Boolean(cfg.isEnabled) });
      return;
    }

    if (cmd === "create_agent") {
      const p = await provider(input.rpcUrl);
      const w = new ethers.Wallet(input.privateKey, p);
      const c = await contract(input.rpcUrl, input.proxyAddress, w);
      const tx = await c.createAgent();
      const rec = await tx.wait();
      out({ txHash: tx.hash, blockNumber: rec ? Number(rec.blockNumber) : null, agent: await w.getAddress() });
      return;
    }

    if (cmd === "allow_agent") {
      const p = await provider(input.rpcUrl);
      const admin = new ethers.Wallet(input.adminPrivateKey, p);
      const c = await contract(input.rpcUrl, input.proxyAddress, admin);
      const tx = await c.allowAgent(input.agent);
      const rec = await tx.wait();
      out({ txHash: tx.hash, blockNumber: rec ? Number(rec.blockNumber) : null, agent: input.agent });
      return;
    }

    if (cmd === "submit_data") {
      const p = await provider(input.rpcUrl);
      const w = new ethers.Wallet(input.privateKey, p);
      const c = await contract(input.rpcUrl, input.proxyAddress, w);
      const dataKey = await c.getSubmissionKey(input.dataLink);
      const tx = await c.submitData(
        input.dataLink,
        input.primaryCategory,
        input.secondaryCategory,
        input.domain,
        input.dataType,
        input.dataFormat,
        BigInt(input.fileSizeInKB)
      );
      const rec = await tx.wait();
      out({
        txHash: tx.hash,
        blockNumber: rec ? Number(rec.blockNumber) : null,
        user: await w.getAddress(),
        dataKey
      });
      return;
    }

    if (cmd === "rate_data") {
      const p = await provider(input.rpcUrl);
      const w = new ethers.Wallet(input.privateKey, p);
      const c = await contract(input.rpcUrl, input.proxyAddress, w);
      const dataKey = await c.getSubmissionKey(input.dataLink);
      const tx = await c.rateData(
        input.dataLink,
        Boolean(input.isSeedDeleted),
        Boolean(input.isValid),
        Boolean(input.hasRating),
        Number(input.rating) || 0,
        Boolean(input.hasSyntheticDataLink),
        input.syntheticDataLink || "",
        Boolean(input.sendTokensImmediately)
      );
      const rec = await tx.wait();
      out({
        txHash: tx.hash,
        blockNumber: rec ? Number(rec.blockNumber) : null,
        agent: await w.getAddress(),
        dataKey
      });
      return;
    }

    if (cmd === "accumulated_credits") {
      const p = await provider(input.rpcUrl);
      const c = await contract(input.rpcUrl, input.proxyAddress, p);
      const credits = await c.accumulatedCredits(input.address);
      out({ credits: credits.toString() });
      return;
    }

    if (cmd === "claim_credits") {
      const p = await provider(input.rpcUrl);
      const w = new ethers.Wallet(input.privateKey, p);
      const c = await contract(input.rpcUrl, input.proxyAddress, w);
      const tx = await c.claimCredits();
      const rec = await tx.wait();
      out({ txHash: tx.hash, blockNumber: rec ? Number(rec.blockNumber) : null, user: await w.getAddress() });
      return;
    }

    fail(`unknown command: ${cmd}`);
  } catch (e) {
    fail(e && e.message ? e.message : String(e));
  }
}

main();

