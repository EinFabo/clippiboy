import assert from "node:assert/strict";
import { test } from "node:test";

import type { Status } from "./client.ts";
import { bufferLabel, durationLabel, recordingLabel } from "./labels.ts";

const base: Status = {
	version: "0.0.0",
	bufferActive: true,
	bufferedSeconds: 45,
	bufferSeconds: 120,
	clipSeconds: 30,
	game: null,
	encoder: null,
	fps: 60,
	droppedFrames: 0,
	saving: false,
	recording: false,
};

test("a recording still being written shows dots, not a stopped key", () =>
	assert.equal(recordingLabel({ ...base, finalizing: true }), "…"));
test("an older ClippiBoy without the flag reads as before", () => assert.equal(recordingLabel(base), ""));
test("a running recording shows its time", () => {
	assert.equal(recordingLabel({ ...base, recording: true, recordingSeconds: 754 }), "12:34");
	assert.equal(recordingLabel({ ...base, recording: true, recordingSeconds: 3723 }), "1:02:03");
});
test("no ClippiBoy is a dash", () => assert.equal(recordingLabel(null), "—"));
test("buffer and duration labels are unchanged", () => {
	assert.equal(bufferLabel(base), "45s");
	assert.equal(bufferLabel(base, 30), "30s");
	assert.equal(durationLabel(150), "2:30");
});
