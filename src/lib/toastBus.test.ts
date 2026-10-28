import { test } from "node:test";
import assert from "node:assert/strict";
import { onToast, toast } from "./toastBus.ts";

test("a toast reaches every listener until it unsubscribes", () => {
  const seen: string[] = [];
  const off = onToast((n) => seen.push(`${n.kind}:${n.message}`));
  toast("ok", "Clip copied");
  off();
  toast("error", "late");
  assert.deepEqual(seen, ["ok:Clip copied"]);
});
