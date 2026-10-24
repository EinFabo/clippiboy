// Runs requests, acceptance, live presence, invisibility and blocking against
// the deployed worker with two test accounts — no Discord needed.
//
//   node scripts/e2e.mjs --seed | npx wrangler d1 execute clippiboy-friends --remote --file /dev/stdin
//   node scripts/e2e.mjs
//   npx wrangler d1 execute clippiboy-friends --remote --command "DELETE FROM users WHERE discord_id LIKE 'test-%'"
//
// The seed makes up fresh tokens and friend codes and keeps them only in
// scripts/e2e-tokens.json (ignored by git). They used to be fixed strings in
// this file — and this file is public, so anybody could sign in as the test
// accounts for as long as they stood. Remove the accounts afterwards all the
// same.
import { createHash, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const BASE = process.env.FRIENDS_SERVER ?? "https://api.clippiboy.com";
const TOKENS = new URL("./e2e-tokens.json", import.meta.url);

if (process.argv.includes("--seed")) {
  const hash = (token) => createHash("sha256").update(token).digest("base64url");
  const code = () => {
    const alphabet = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    return Array.from(randomBytes(8), (byte) => alphabet[byte % alphabet.length]).join("");
  };
  const seed = {
    a: { token: randomBytes(32).toString("base64url"), id: "00000000-0000-4000-8000-00000000000a" },
    b: { token: randomBytes(32).toString("base64url"), id: "00000000-0000-4000-8000-00000000000b" },
  };
  writeFileSync(TOKENS, JSON.stringify(seed, null, 2));
  const now = Date.now();
  console.log(`INSERT INTO users (id, discord_id, username, display_name, avatar, friend_code, created_at) VALUES
  ('${seed.a.id}', 'test-a', 'testalpha', 'Test Alpha', NULL, '${code()}', ${now}),
  ('${seed.b.id}', 'test-b', 'testbravo', 'Test Bravo', NULL, '${code()}', ${now});
INSERT INTO sessions (token_hash, user_id, created_at, last_used) VALUES
  ('${hash(seed.a.token)}', '${seed.a.id}', ${now}, ${now}),
  ('${hash(seed.b.token)}', '${seed.b.id}', ${now}, ${now});`);
  process.exit(0);
}
const { a: A, b: B } = JSON.parse(readFileSync(TOKENS, "utf8"));

async function api(who, method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: { Authorization: `Bearer ${who.token}`, "Content-Type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await res.json().catch(() => null);
  return { status: res.status, data };
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
function check(label, ok, extra) {
  console.log(`${ok ? "OK  " : "FAIL"} ${label}${ok ? "" : " " + JSON.stringify(extra)}`);
  if (!ok) failures++;
}

function socket(who, name) {
  const ws = new WebSocket(BASE.replace(/^http/, "ws") + "/ws", {
    headers: { Authorization: `Bearer ${who.token}` },
  });
  const inbox = [];
  ws.onmessage = (e) => inbox.push(e.data === "pong" ? "pong" : JSON.parse(e.data));
  ws.onerror = (e) => console.log(name, "error", e.message);
  return new Promise((resolve) => (ws.onopen = () => resolve({ ws, inbox })));
}
async function waitFor(inbox, pred, ms = 5000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const hit = inbox.find(pred);
    if (hit) {
      inbox.splice(inbox.indexOf(hit), 1);
      return hit;
    }
    await sleep(100);
  }
  return null;
}

// Clean slate between runs.
await api(A, "DELETE", `/friends/${B.id}`);
await api(A, "DELETE", `/blocks/${B.id}`);
await api(B, "DELETE", `/blocks/${A.id}`);

check("no token → 401", (await fetch(BASE + "/me")).status === 401);
const me = await api(A, "GET", "/me");
check("GET /me", me.status === 200 && /^[A-Z2-9]{8}$/.test(me.data.friendCode), me);
check("self request refused", (await api(A, "POST", "/friends/request", { query: me.data.friendCode })).status === 400);
check("unknown refused", (await api(A, "POST", "/friends/request", { query: "nobody-here" })).status === 404);

const sa = await socket(A, "A");
const sb = await socket(B, "B");
sa.ws.send(JSON.stringify({ t: "hello", invisible: false, game: null, since: null }));
sb.ws.send(JSON.stringify({ t: "hello", invisible: false, game: "VALORANT", since: Date.now() - 60000 }));
check("A snapshot", !!(await waitFor(sa.inbox, (m) => m.t === "snapshot")));
check("B snapshot", !!(await waitFor(sb.inbox, (m) => m.t === "snapshot")));

const bravo = (await api(B, "GET", "/me")).data;
const req = await api(A, "POST", "/friends/request", { query: bravo.friendCode.toLowerCase() });
check("request by code", req.status === 200 && req.data.status === "sent", req);
check("B told about request", !!(await waitFor(sb.inbox, (m) => m.t === "friends")));
const listB = await api(B, "GET", "/friends");
check("B sees incoming", listB.data.incoming.length === 1 && listB.data.incoming[0].id === A.id, listB);
check("duplicate refused", (await api(A, "POST", "/friends/request", { query: "testbravo" })).status === 409);
check("A cannot accept own request", (await api(A, "POST", `/friends/${B.id}/accept`)).status === 404);

check("B accepts", (await api(B, "POST", `/friends/${A.id}/accept`)).status === 200);
const snapA = await waitFor(sa.inbox, (m) => m.t === "snapshot" && B.id in m.friends);
check("A snapshot shows B in VALORANT", snapA?.friends[B.id]?.game === "VALORANT", snapA);

sb.ws.send(JSON.stringify({ t: "presence", invisible: false, game: "Rocket League", since: Date.now() }));
const live = await waitFor(sa.inbox, (m) => m.t === "presence" && m.id === B.id);
check("A gets B's game change live", live?.presence?.game === "Rocket League", live);

sb.ws.send(JSON.stringify({ t: "presence", invisible: true, game: "Rocket League", since: Date.now() }));
const hidden = await waitFor(sa.inbox, (m) => m.t === "presence" && m.id === B.id);
check("invisible B looks offline", hidden && hidden.presence === null, hidden);
check("going invisible stamps last seen", typeof hidden?.lastSeen === "number", hidden);

sa.ws.send(JSON.stringify({ t: "relay", to: B.id, body: { kind: "shareCancel", id: "probe" } }));
const probe = await waitFor(sa.inbox, (m) => m.t === "relayFailed" && m.ref === "probe");
check("invisible B looks offline to a relay too", probe?.reason === "offline", probe);

sb.ws.send(JSON.stringify({ t: "relay", to: A.id, body: { kind: "shareCancel", id: "hi" } }));
const hi = await waitFor(sa.inbox, (m) => m.t === "relay" && m.body?.id === "hi");
check("invisible B can still write to A", hi?.from === B.id, hi);

sa.ws.send(JSON.stringify({ t: "relay", to: B.id, body: { kind: "shareCancel", id: "answer" } }));
const answer = await waitFor(sb.inbox, (m) => m.t === "relay" && m.body?.id === "answer");
check("a friend invisible B wrote to still reaches B", answer?.from === A.id, answer);

sb.ws.send(JSON.stringify({ t: "presence", invisible: false, game: null, since: null }));
const back = await waitFor(sa.inbox, (m) => m.t === "presence" && m.id === B.id);
check("B online without game", back?.presence && back.presence.game === null, back);

sb.ws.send(JSON.stringify({ t: "presence", invisible: false, game: null, since: null, status: `  back at  ${"x".repeat(80)}`, busy: true }));
const busy = await waitFor(sa.inbox, (m) => m.t === "presence" && m.id === B.id);
check("status text is cut to 60", busy?.presence?.status?.length === 60 && busy.presence.status.startsWith("back at x"), busy);
check("B is busy", busy?.presence?.busy === true, busy);

const offer = { kind: "share-offer", id: "o1", name: "Ace", size: 1234 };
sa.ws.send(JSON.stringify({ t: "relay", to: B.id, body: offer }));
const relayed = await waitFor(sb.inbox, (m) => m.t === "relay");
check("relay reaches the friend", relayed?.from === A.id && relayed.body.id === "o1", relayed);
sa.ws.send(JSON.stringify({ t: "relay", to: B.id, body: { id: "big", pad: "x".repeat(5000) } }));
const big = await waitFor(sa.inbox, (m) => m.t === "relayFailed");
check("relay refuses big bodies", big?.reason === "too big" && big.ref === "big", big);
sa.ws.send(JSON.stringify({ t: "relay", to: "00000000-0000-4000-8000-0000000000ff", body: { id: "x" } }));
const stranger = await waitFor(sa.inbox, (m) => m.t === "relayFailed");
check("relay refuses strangers", stranger?.reason === "not a friend", stranger);

sa.ws.send("ping");
check("ping → pong", !!(await waitFor(sa.inbox, (m) => m === "pong")));

sb.ws.close();
const gone = await waitFor(sa.inbox, (m) => m.t === "presence" && m.id === B.id);
check("B closing → offline", gone && gone.presence === null, gone);
check("last seen moves on", gone?.lastSeen >= hidden?.lastSeen, gone);
sa.ws.send(JSON.stringify({ t: "relay", to: B.id, body: { id: "o2" } }));
const offline = await waitFor(sa.inbox, (m) => m.t === "relayFailed");
check("relay to offline friend says so", offline?.reason === "offline" && offline.ref === "o2", offline);

const sa2 = await socket(A, "A2");
sa2.ws.send(JSON.stringify({ t: "hello", invisible: false, game: null, since: null }));
const snap2 = await waitFor(sa2.inbox, (m) => m.t === "snapshot");
check("snapshot carries last seen", snap2?.lastSeen?.[B.id] === gone?.lastSeen, snap2);
sa2.ws.close();

const sb2 = await socket(B, "B2");
sb2.ws.send(JSON.stringify({ t: "hello", invisible: false, game: null, since: null }));
await waitFor(sb2.inbox, (m) => m.t === "snapshot");
for (let i = 0; i < 15; i++) sb2.ws.send(JSON.stringify({ t: "presence", invisible: false, game: null, since: null, status: `s${i}` }));
let lastStatus = null;
for (const end = Date.now() + 35_000; Date.now() < end && lastStatus !== "s14"; ) {
  const m = await waitFor(sa.inbox, (x) => x.t === "presence" && x.id === B.id, 1000);
  if (m) lastStatus = m.presence?.status;
}
check("a burst of presence changes ends on the last one", lastStatus === "s14", lastStatus);
sb2.ws.close();

let closeCode = null;
sa.ws.onclose = (e) => (closeCode = e.code);
const more = [];
for (let i = 0; i < 4; i++) more.push(await socket(A, "A+" + i));
for (const end = Date.now() + 5000; Date.now() < end && closeCode === null; ) await sleep(100);
check("a fifth connection closes the oldest", closeCode === 4002, closeCode);
for (const m of more) m.ws.close();

check("B blocks A", (await api(B, "POST", `/blocks/${A.id}`)).status === 200);
const listA = await api(A, "GET", "/friends");
check("friendship gone after block", listA.data.friends.length === 0, listA);
const again = await api(A, "POST", "/friends/request", { query: "testbravo" });
check("blocked A sees 'not found'", again.status === 404, again);
const listB2 = await api(B, "GET", "/friends");
check("B's block list has A", listB2.data.blocked[0]?.id === A.id, listB2);
check("B unblocks", (await api(B, "DELETE", `/blocks/${A.id}`)).status === 200);

check("exchange with junk refused", (await fetch(BASE + "/auth/exchange", { method: "POST", body: JSON.stringify({ code: "x", verifier: "y" }) })).status === 400);
const start = await fetch(BASE + "/auth/start?challenge=" + "a".repeat(43), { redirect: "manual" });
check("auth/start sends the browser to Discord", start.status === 302 && start.headers.get("location")?.startsWith("https://discord.com/"), start.status);

sa.ws.close();
console.log(failures === 0 ? "ALL PASSED" : `${failures} FAILED`);
process.exit(failures ? 1 : 0);
