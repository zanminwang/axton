import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, symlink, link } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, execFile } from "node:child_process";
import { promisify } from "node:util";
import { createRequire } from "node:module";
import { Client } from "../../packages/frontend/client-js/index.mts";
import { Bridge } from "../../packages/frontend/client-js/bindings/bridge.mts";
import {
  GeneratedClient,
  schema,
  liveModels,
  makeMutations,
} from "./client.ts";
import { GeneratedClient as UpgradedClient } from "./upgraded/client.ts";
import { GeneratedClient as VersionedClient } from "./versioned/client.ts";
import { host } from "./server.mjs";
import { isRetryableTransactionError } from "../../packages/backend/server/bindings/retryable.mts";

const assertReportedExhaustion = (h) => {
  const escaped = h.transactions.filter((tx) => tx.outcome === "escaped");
  assert.equal(
    h.rawErrors.length,
    escaped.length,
    "each exhausted transaction is reported once",
  );
  for (const tx of escaped)
    assert.equal(
      h.rawErrors.filter((error) => error === tx.error).length,
      1,
      "each escape reports the same raw object exactly once",
    );
  for (const error of h.rawErrors) {
    const matches = escaped.filter((tx) => tx.error === error);
    assert.equal(
      matches.length,
      1,
      "reported error is the original exhausted transaction error",
    );
    assert.equal(
      matches[0].attempts,
      4,
      "default adapter budget: initial attempt plus three retries",
    );
    assert.equal(
      isRetryableTransactionError(error),
      true,
      "handler/nonretryable failures are not allowed",
    );
  }
};
const until = async (probe, label) => {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    if (await probe()) return;
    await new Promise((r) => setTimeout(r, 20));
  }
  throw Error(`Timed out: ${label}`);
};
const sqliteFault = (path, sql) =>
  promisify(execFile)("python3", [
    "-c",
    "import sqlite3,sys;db=sqlite3.connect(sys.argv[1]);db.executescript(sys.argv[2]);db.close()",
    path,
    sql,
  ]);
const open = (path, url, owner = "alice") =>
  GeneratedClient.open({
    path,
    stream: `User:${owner}`,
    ...(url ? { connection: { url, token: owner } } : {}),
  });
const publish = (client, id, text, mode = "normal") =>
  client.mutations.publish({ entry: { id, text }, call: mode });

if (process.argv[2] === "enqueue-child") {
  const client = await open(process.argv[3]);
  await client.transaction(async (tx) => {
    await tx.models.draft.create({ id: "restart-companion", text: "keep" });
    await tx.mutations.publish({
      entry: { id: "restart-entry", text: " normalized restart " },
      call: "private",
    });
  });
  // Exit without close: WAL + committed optimism + queue survive the missing waiter.
  process.exit(73);
} else if (process.argv[2] === "accepted-child") {
  // A pending Call.wait() does not hold the Node event loop, and the push now
  // follows the handshake; stay alive until the parent kills this process.
  setInterval(() => {}, 1000);
  const client = await open(process.argv[3], process.argv[4], "killed");
  await (await publish(client, "killed-accepted", " accepted ")).wait();
  throw Error("accepted response should remain held");
} else {
  test("P7: unsupported name/version is a durable member refusal over real HTTP, with exact replay and next Batch", async () => {
    for (const [name, version, owner] of [
      ["Publish", 2, "p7-version"],
      ["RetiredPublish", 1, "p7-name"],
    ]) {
      const h = await host(),
        dir = await mkdtemp(join(tmpdir(), "axton-p7-"));
      const retained = structuredClone(schema);
      const descriptor = structuredClone(
        retained.actions.find((a) => a.name === "Publish"),
      );
      descriptor.name = name;
      descriptor.version = version;
      retained.actions.push(descriptor);
      const path = join(dir, "db");
      let client;
      try {
        client = await Client.open({
          path,
          schema: retained,
          stream: `User:${owner}`,
        });
        const submit = (name, version, suffix) =>
          client.submitMutation(
            name,
            version,
            {
              entry: { id: `${owner}-${suffix}`, text: suffix },
              call: "normal",
            },
            (value) => value,
          );
        const before = await submit("Publish", 1, "before");
        const unsupported = await submit(name, version, "unsupported");
        const after = await submit("Publish", 1, "after");
        h.loseNext();
        await client.connect({ url: h.url, token: owner });
        const waiting = Promise.all([
          before.wait(),
          unsupported.wait(),
          after.wait(),
        ]);
        await until(
          () =>
            before.status === "succeeded" &&
            unsupported.status === "failed" &&
            after.status === "succeeded",
          "P7 durable member outcomes",
        );
        const outcomes = await waiting;
        assert.equal(outcomes[0].error, null);
        assert.equal(outcomes[1].error.code, "mutation_version_unsupported");
        assert.equal(outcomes[2].error, null);
        assert.deepEqual(h.executions, [`${owner}-before`, `${owner}-after`]);
        assert.ok(h.batches.length >= 2);
        assert.equal(h.batches[1], h.batches[0]);
        const frozen = JSON.parse(h.batches[0]);
        assert.equal(frozen.mutations.length, 3);
        const post = async (body) => {
          const response = await fetch(`${h.url}/sync/mutations`, {
            method: "POST",
            headers: {
              authorization: `Bearer ${owner}`,
              "content-type": "application/json",
            },
            body,
          });
          return { status: response.status, body: await response.json() };
        };
        const replay = await post(h.batches[0]);
        assert.equal(replay.status, 200);
        assert.equal(
          replay.body.results[1].outcome.code,
          "mutation_version_unsupported",
        );
        assert.equal(replay.body.results[0].outcome.kind, "accepted");
        assert.equal(replay.body.results[2].outcome.kind, "accepted");
        assert.deepEqual(await post(h.batches[0]), replay);
        assert.deepEqual(h.executions, [`${owner}-before`, `${owner}-after`]);
        assert.deepEqual(await post("not JSON"), {
          status: 400,
          body: { code: "request.invalid" },
        });
        const corrupt = { ...frozen, digest: "0".repeat(64) };
        assert.deepEqual(await post(JSON.stringify(corrupt)), {
          status: 400,
          body: { code: "request.invalid" },
        });
        assert.deepEqual(h.executions, [`${owner}-before`, `${owner}-after`]);
        assert.equal(
          await client.read("Entry", { id: `${owner}-unsupported` }),
          null,
        );
        assert.equal(
          (await client.read("Entry", { id: `${owner}-before` })).text,
          "before",
        );
        assert.equal(
          (await client.read("Entry", { id: `${owner}-after` })).text,
          "after",
        );
        await client.close();
        client = await Client.open({
          path,
          schema: retained,
          stream: `User:${owner}`,
        });
        assert.equal(
          (await client.rejections.get(frozen.mutations[1].id)).code,
          "mutation_version_unsupported",
        );
        assert.equal((await client.syncState()).pending, 0);
        const next = await submit("Publish", 1, "next");
        await client.connect({ url: h.url, token: owner });
        assert.equal((await next.wait()).error, null);
        assert.equal(JSON.parse(h.batches.at(-1)).batchId, frozen.batchId + 1);
        assert.deepEqual(h.executions, [
          `${owner}-before`,
          `${owner}-after`,
          `${owner}-next`,
        ]);
        assertReportedExhaustion(h);
      } finally {
        await client?.close();
        await h.close();
        await rm(dir, { recursive: true, force: true });
      }
    }
  });
  test("exhaustion oracle refuses unmatched, nonretryable and duplicate fault reports", () => {
    const retryable = Object.assign(new Error("serialization"), {
      code: "40001",
    });
    const handler = new Error("handler defect");
    const escape = (error, attempts = 4) => ({
      outcome: "escaped",
      error,
      attempts,
    });
    const check = (rawErrors, transactions) =>
      assertReportedExhaustion({ rawErrors, transactions });
    assert.throws(() => check([retryable], []));
    assert.throws(() => check([], [escape(retryable)]));
    assert.throws(() => check([handler], [escape(handler)]));
    assert.throws(() => check([retryable], [escape(retryable, 1)]));
    assert.throws(() =>
      check([retryable, retryable], [escape(retryable), escape(handler)]),
    );
    assert.throws(() =>
      check([retryable], [escape(retryable), escape(retryable)]),
    );
  });
  test("bounded transaction exhaustion reports the original fault over HTTP and the SDK recovers", async () => {
    const failures = Array.from({ length: 4 }, () =>
      Object.assign(new Error("injected transaction serialization conflict"), {
        code: "40001",
      }),
    );
    let failedRequestState;
    const h = await host({
      transactionFailures: failures,
      onTransactionEscape: async ({ pool, error }) => {
        if (error !== failures[3]) return;
        failedRequestState = (
          await pool.query(`SELECT
          (SELECT count(*) FROM axton_store WHERE principal='exhaustion')::int AS stores,
          (SELECT count(*) FROM axton_mutation_result r JOIN axton_store s ON s.id=r.store_id WHERE s.principal='exhaustion')::int AS results,
          (SELECT count(*) FROM sdk05_entry WHERE id='exhaustion-entry')::int AS entries`)
        ).rows[0];
      },
    });
    const dir = await mkdtemp(join(tmpdir(), "axton-exhaustion-"));
    let client;
    try {
      client = await open(join(dir, "db"), undefined, "exhaustion");
      const call = await publish(
        client,
        "exhaustion-entry",
        "recover",
        "private",
      );
      await client.connect({ url: h.url, token: "exhaustion" });
      await until(
        () => call.status === "succeeded",
        "SDK recovers after transaction exhaustion",
      );
      assert.equal((await call.wait()).error, null);
      assert.equal(call.status, "succeeded");
      assert.deepEqual(h.executions, ["exhaustion-entry"]);
      assert.equal((await client.syncState()).pending, 0);
      const exhausted = h.transactions.find((tx) => tx.error === failures[3]);
      assert.equal(exhausted.attempts, 4);
      assert.equal(exhausted.outcome, "escaped");
      assert.deepEqual(failedRequestState, {
        stores: 0,
        results: 0,
        entries: 0,
      });
      assert.equal(
        h.rawErrors.filter((error) => error === failures[3]).length,
        1,
      );
      assert.ok(
        !h.rawErrors.includes(failures[0]) &&
          !h.rawErrors.includes(failures[1]) &&
          !h.rawErrors.includes(failures[2]),
      );
      const response = h.responses.find((response) => response.status === 500);
      assert.equal(response.route, "/sync/handshake");
      assert.deepEqual(JSON.parse(response.body), { code: "server" });
      assert.ok(
        h.responses.some(
          (response) =>
            response.route === "/sync/handshake" && response.status === 200,
        ),
      );
      assertReportedExhaustion(h);
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("ported reads: SIGKILL after cloud acceptance replays identical frozen Batch without repeating execution", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-killed-accepted-"));
    let child, client;
    try {
      h.holdAcknowledgements();
      const path = join(dir, "db");
      child = spawn(
        process.execPath,
        [new URL(import.meta.url).pathname, "accepted-child", path, h.url],
        { stdio: ["ignore", "pipe", "pipe"] },
      );
      let errors = "";
      child.stderr.on("data", (bytes) => (errors += bytes));
      const exited = new Promise((resolve) =>
        child.once("exit", (code, signal) => resolve({ code, signal })),
      );
      await until(
        () => h.heldAcknowledgementCount > 0,
        "accepted cloud response held",
      );
      assert.ok(h.executions.length > 0);
      assert.ok(h.executions.every((id) => id === "killed-accepted"));
      // SQL may retry before acceptance; replay must add no handler attempt afterward.
      const acceptedExecutions = [...h.executions];
      const frozen = h.batches[0];
      child.kill("SIGKILL");
      assert.deepEqual(await exited, { code: null, signal: "SIGKILL" }, errors);
      h.releaseAcknowledgements();
      client = await open(path, h.url, "killed");
      await until(
        async () => (await client.syncState()).pending === 0,
        "killed client recovery",
      );
      assert.ok(h.batches.length >= 2);
      assert.equal(h.batches.at(-1), frozen);
      assert.deepEqual(h.executions, acceptedExecutions);
      assert.equal(
        (await client.models.entry.get({ id: "killed-accepted" })).text,
        "accepted",
      );
      assert.equal(
        (
          await h.pool.query(
            "SELECT count(*)::int n FROM sdk05_entry WHERE id='killed-accepted'",
          )
        ).rows[0].n,
        1,
      );
    } finally {
      if (child?.exitCode === null && child.signalCode === null)
        child.kill("SIGKILL");
      h.releaseAcknowledgements();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("ported reads: multi-Model Query Loader failure installs neither Model; false and nullable arguments stay read-only", async () => {
    const h = await host({ mixed: true }),
      dir = await mkdtemp(join(tmpdir(), "axton-mixed-read-"));
    let client;
    try {
      await h.pool.query(
        "INSERT INTO sdk05_entry VALUES('mixed-entry','entry','mixed'),('mixed-snapshot','snapshot','mixed')",
      );
      client = await Client.open({
        schema: h.mixedSchema,
        path: join(dir, "db"),
        stream: "User:mixed",
        connection: { url: h.url, token: "mixed" },
      });
      const query = (store) =>
        client.invokeQuery("Mixed", 1, { id: null }, (value) => value, {
          store,
        });
      h.failSnapshot(true);
      await assert.rejects(query(true));
      for (const model of ["Entry", "Snapshot"])
        assert.deepEqual(await client.querySpec(model), []);
      h.failSnapshot(false);
      const result = await query(false);
      assert.equal(result.entry.text, "entry");
      assert.equal(result.snapshot.text, "snapshot");
      for (const model of ["Entry", "Snapshot"])
        assert.deepEqual(await client.querySpec(model), []);
      assert.deepEqual(await query(true), result);
      for (const model of ["Entry", "Snapshot"])
        assert.equal((await client.querySpec(model)).length, 1);
      assert.equal(
        (
          await h.pool.query(
            "SELECT count(*)::int n FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id WHERE r.identity->>'id' IN ('mixed-entry','mixed-snapshot')",
          )
        ).rows[0].n,
        0,
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("Batch ownership: an undeliverable historical target settles from its receipt before the next Batch", async () => {
    const h = await host({ unbootstrapped: true }),
      dir = await mkdtemp(join(tmpdir(), "axton-batch-window-"));
    let client;
    try {
      await h.backend.transaction(async ({ tx, streams }) => {
        await tx.query(
          "INSERT INTO sdk05_entry VALUES('window-owned','canonical','window'),('window-independent','independent','window')",
        );
        streams(["User:window"]).track.entry("window-owned");
      });
      const path = join(dir, "db");
      client = await Client.open({
        path,
        schema: h.mixedSchema,
        stream: "User:window",
        connection: { url: h.url, token: "window" },
      });
      await until(
        async () =>
          (await client.readSql("SELECT start_cursor FROM axton_store"))[0]
            .start_cursor !== null,
        "handshake prefix",
      );
      h.holdAcknowledgements();
      await makeMutations(client).publish({
        entry: { id: "window-owned", text: "optimistic" },
        call: "noop",
      });
      await until(
        () => h.heldAcknowledgementCount > 0,
        "first Batch acknowledgement parked",
      );
      await makeMutations(client).publish({
        entry: { id: "window-independent", text: "second optimism" },
        call: "noop",
      });
      // The queued call cannot freeze while the first Batch is unsettled.
      await client.close();
      client = undefined;
      h.releaseAcknowledgements();
      client = await Client.open({
        path,
        schema: h.mixedSchema,
        stream: "User:window",
        connection: { url: h.url, token: "window" },
      });
      await until(
        async () => (await client.syncState()).pending === 0,
        "both disjoint calls settle",
      );
      const first = (
        await client.readSql(
          "SELECT batch_id,targets FROM axton_mutation_queue WHERE id=1",
        )
      )[0];
      // window-owned sits at or below S in a Model this Store never
      // Bootstraps: the Stream cannot deliver it, so the receipt finalizes it.
      assert.equal(JSON.parse(first.targets)[0].kind, "stream");
      assert.equal(
        (await liveModels(client).entry.get({ id: "window-owned" })).text,
        "canonical",
      );
      assert.equal(
        (await liveModels(client).entry.get({ id: "window-independent" })).text,
        "independent",
      );
      assert.deepEqual(
        h.requests
          .map((r) => r.route)
          .filter(
            (route) =>
              !["/sync/handshake", "/sync/mutations", "/sync/pull"].includes(route),
          ),
        [],
        "settlement uses only handshake, push and pull",
      );
    } finally {
      h.releaseAcknowledgements();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("ported reads: private Model-target no-op settles optimism to existing canonical state without enrollment", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-private-noop-"));
    let client;
    try {
      await h.pool.query(
        "INSERT INTO sdk05_entry VALUES('private-noop','existing canonical','noop')",
      );
      client = await open(join(dir, "db"), undefined, "noop");
      const call = await publish(
        client,
        "private-noop",
        "optimistic value",
        "noop",
      );
      assert.equal(
        (await client.models.entry.get({ id: "private-noop" })).text,
        "optimistic value",
      );
      await client.connect({ url: h.url, token: "noop" });
      const result = await call.wait();
      assert.equal(result.error, null);
      assert.equal(result.result.entry.text, "existing canonical");
      assert.equal(
        (await client.models.entry.get({ id: "private-noop" })).text,
        "existing canonical",
      );
      assert.equal((await client.syncState()).pending, 0);
      assert.equal(
        (
          await h.pool.query(
            "SELECT text FROM sdk05_entry WHERE id='private-noop'",
          )
        ).rows[0].text,
        "existing canonical",
      );
      assert.equal(
        (
          await h.pool.query(
            "SELECT count(*)::int n FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id WHERE r.identity->>'id'='private-noop'",
          )
        ).rows[0].n,
        0,
      );
      const outcome = (
        await h.pool.query(
          "SELECT result FROM axton_mutation_result WHERE store_id=$1",
          [client.clientId],
        )
      ).rows[0].result.outcome;
      assert.equal(outcome.kind, "accepted");
      assert.equal(outcome.syncCursor, 0);
      assert.equal(outcome.targets[0].kind, "private");
      assert.equal(outcome.targets[0].record.cursor, null);
      assert.equal(outcome.targets[0].record.state.text, "existing canonical");
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A1/A2/A4/A8/A11/A15: offline Batch, partial refusal, lost response, local ownership and multi-device completion", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-"));
    let a, b;
    try {
      a = await open(join(dir, "a"));
      b = await open(join(dir, "b"));
      assert.notEqual(a.clientId, b.clientId);
      let accepted, refused, privateCall;
      await a.transaction(async (tx) => {
        accepted = await tx.mutations.publish({
          entry: { id: "joined-accepted", text: " canonical " },
          call: "multi",
        });
        refused = await tx.mutations.publish({
          entry: { id: "joined-refused", text: "refuse" },
          call: "normal",
        });
        privateCall = await tx.mutations.publish({
          entry: { id: "joined-private", text: " private " },
          call: "private",
        });
      });
      assert.equal(
        (await a.models.entry.get({ id: "joined-accepted" })).text,
        " canonical ",
      );
      await a.models.entry.update(
        { id: "joined-private" },
        { text: "later direct" },
      );
      h.loseNext();
      await b.connect({ url: h.url, token: "alice" });
      await b.bootstrap();
      await a.connect({ url: h.url, token: "alice" });
      const [yes, no, priv] = await Promise.all([
        accepted.wait(),
        refused.wait(),
        privateCall.wait(),
      ]);
      assert.equal(yes.error, null);
      assert.equal(yes.result.entry.text, "canonical");
      assert.equal(no.error.code, "publish.refused");
      assert.equal(priv.result.entry.text, "private");
      assert.equal(await a.models.entry.get({ id: "joined-refused" }), null);
      assert.equal(
        (await a.models.entry.get({ id: "joined-private" })).text,
        "later direct",
      );
      assert.equal(h.batches.length >= 2, true);
      assert.equal(
        h.batches[0],
        h.batches[1],
        "lost response must retry exact Batch bytes",
      );
      assert.deepEqual(h.executions, [
        "joined-accepted",
        "joined-refused",
        "joined-private",
      ]);
      const submitted = JSON.parse(h.batches[0]);
      assert.equal(submitted.protocol, 6);
      assert.equal(submitted.mutations.length, 3);
      await b.bootstrap();
      // A's receipt and B's initial Bootstrap do not fence B's live delivery.
      await until(
        async () =>
          (await b.models.entry.get({ id: "joined-accepted" }))?.text ===
          "canonical",
        "device B receives accepted Entry",
      );
      assert.equal(
        (await b.models.entry.get({ id: "joined-accepted" })).text,
        "canonical",
      );
      assert.equal(
        await b.models.entry.get({ id: "joined-private" }),
        null,
        "private settlement did not enroll another device",
      );
      const members = (
        await h.pool.query(
          "SELECT s.stream,s.cursor,r.identity FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id WHERE r.identity->>'id' LIKE 'joined-%' ORDER BY s.stream,s.cursor",
        )
      ).rows;
      assert.deepEqual(
        members.map((x) => [x.stream, x.identity.id]),
        [
          ["User:alice", "joined-accepted"],
          ["User:bob", "joined-accepted"],
        ],
      );
      const other = await publish(b, "device-b-entry", "device b");
      assert.equal((await other.wait()).error, null);
      assert.equal(JSON.parse(h.batches.at(-1)).storeId, b.clientId);
    } finally {
      await a?.close();
      await b?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A3/A10: child exits after local commit, reopened Store settles without original waiter", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-reopen-"));
    let client;
    try {
      const path = join(dir, "db");
      const child = spawn(
        process.execPath,
        [new URL(import.meta.url).pathname, "enqueue-child", path],
        { stdio: ["ignore", "pipe", "pipe"] },
      );
      let error = "";
      child.stderr.on("data", (bytes) => (error += bytes));
      assert.equal(
        await new Promise((resolve) => child.on("exit", resolve)),
        73,
        error,
      );
      client = await open(path);
      const id = client.clientId;
      assert.equal(
        (await client.models.entry.get({ id: "restart-entry" })).text,
        " normalized restart ",
      );
      assert.equal(
        (await client.models.draft.get({ id: "restart-companion" })).text,
        "keep",
      );
      await client.connect({ url: h.url, token: "alice" });
      await until(
        async () =>
          (
            await client.readSql(
              "SELECT reconciled FROM axton_mutation_queue WHERE id=1",
            )
          )[0]?.reconciled === 1,
        "reopened durable settlement",
      );
      assert.equal(
        (await client.models.entry.get({ id: "restart-entry" })).text,
        "normalized restart",
      );
      await client.close();
      client = await open(path);
      assert.equal(client.clientId, id);
      const persisted = await client.readSql(
        "SELECT result,reconciled FROM axton_mutation_queue WHERE id=1",
      );
      assert.equal(persisted[0].reconciled, 1);
      assert.equal(
        JSON.parse(persisted[0].result).entry.text,
        "normalized restart",
      );
      assert.deepEqual(h.executions, ["restart-entry"]);
      await client.close();
      client = undefined;
      const native = createRequire(import.meta.url)(
        "../../bindings/node/axton-node.node",
      );
      const { bridge } = await Bridge.open(native, {
        path,
        schema,
        stream: "User:alice",
      });
      try {
        const completed = await bridge.task({
          kind: "callCompletion",
          callId: `${id}:1`,
        });
        assert.equal(completed.outcome.status, "succeeded");
        assert.equal(completed.outcome.result.entry.text, "normalized restart");
      } finally {
        await bridge.close();
      }
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A3: actual SQLite acknowledgement failure retries unchanged Batch without duplicate cloud execution", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-ack-fault-"));
    let client;
    try {
      const path = join(dir, "db");
      client = await open(path);
      const call = await publish(
        client,
        "ack-fault-entry",
        " canonical ack ",
        "private",
      );
      await sqliteFault(
        path,
        "CREATE TRIGGER task7_ack_failure BEFORE UPDATE OF last_acknowledged_batch_id ON axton_store BEGIN SELECT RAISE(ABORT,'task7 acknowledgement commit failure');END;",
      );
      await client.connect({ url: h.url, token: "alice" });
      await until(
        () => h.batches.length >= 2,
        "acknowledgement failure causes exact retry",
      );
      assert.equal(h.batches[0], h.batches[1]);
      assert.deepEqual(h.executions, ["ack-fault-entry"]);
      assert.equal(
        (
          await client.readSql(
            "SELECT last_acknowledged_batch_id FROM axton_store",
          )
        )[0].last_acknowledged_batch_id,
        0,
      );
      assert.equal(
        (
          await client.readSql(
            "SELECT sync_cursor FROM axton_mutation_queue WHERE id=1",
          )
        )[0].sync_cursor,
        null,
      );
      await sqliteFault(path, "DROP TRIGGER task7_ack_failure;");
      assert.equal((await call.wait()).result.entry.text, "canonical ack");
      assert.deepEqual(h.executions, ["ack-fault-entry"]);
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A3/A4: saved acceptance survives SQLite settlement failure and reopening without original waiter", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-settle-fault-"));
    let client;
    try {
      const path = join(dir, "db");
      client = await open(path);
      const id = client.clientId;
      await publish(
        client,
        "settle-fault-entry",
        " canonical settle ",
        "private",
      );
      await client.models.entry.update(
        { id: "settle-fault-entry" },
        { text: "later local" },
      );
      await sqliteFault(
        path,
        "CREATE TRIGGER task7_settlement_failure BEFORE UPDATE OF reconciled ON axton_mutation_queue WHEN NEW.reconciled=1 BEGIN SELECT RAISE(ABORT,'task7 settlement commit failure');END;",
      );
      await client.connect({ url: h.url, token: "alice" });
      await until(
        async () =>
          (
            await client.readSql(
              "SELECT last_acknowledged_batch_id FROM axton_store",
            )
          )[0].last_acknowledged_batch_id === 1,
        "saved acceptance boundary",
      );
      const saved = (
        await client.readSql(
          "SELECT sync_cursor,result,reconciled FROM axton_mutation_queue WHERE id=1",
        )
      )[0];
      assert.equal(saved.reconciled, 0);
      assert.equal(JSON.parse(saved.result).entry.text, "canonical settle");
      assert.equal(
        (await client.models.entry.get({ id: "settle-fault-entry" })).text,
        "later local",
      );
      await client.close();
      client = undefined;
      await sqliteFault(path, "DROP TRIGGER task7_settlement_failure;");
      client = await open(path, h.url);
      assert.equal(client.clientId, id);
      await until(
        async () =>
          (
            await client.readSql(
              "SELECT reconciled FROM axton_mutation_queue WHERE id=1",
            )
          )[0].reconciled === 1,
        "reopened settlement",
      );
      assert.equal(
        (await client.models.entry.get({ id: "settle-fault-entry" })).text,
        "later local",
      );
      assert.deepEqual(h.executions, ["settle-fault-entry"]);
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A5/A9/A12/A13: zero Bootstrap, fresh Query snapshots, direct-delete refill and Store aliases", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-reads-"));
    let client;
    try {
      const path = join(dir, "db");
      client = await open(path, h.url, "reader");
      await client.bootstrap();
      const progress = await client.readSql(
        "SELECT start_cursor,bootstrap_cursor,cursor FROM axton_store",
      );
      assert.equal(progress[0].start_cursor, 0);
      assert.equal(progress[0].bootstrap_cursor, 0);
      assert.equal(progress[0].cursor, 0);
      for (const [alias, make] of [
        ["symlink", symlink],
        ["hardlink", link],
      ]) {
        const name = join(dir, alias);
        await make(path, name);
        await assert.rejects(
          open(name, h.url, "reader"),
          /in.use|locked|already|ownership/,
        );
      }
      await h.pool.query(
        "INSERT INTO sdk05_entry VALUES('snapshot-read','snapshot','reader')",
      );
      assert.equal(
        (await client.queries.find({ id: "snapshot-read" }, { store: false }))
          .entry.text,
        "snapshot",
      );
      assert.equal(
        await client.models.entry.get({ id: "snapshot-read" }),
        null,
      );
      await h.pool.query(
        "UPDATE sdk05_entry SET text='fresh' WHERE id='snapshot-read'",
      );
      assert.equal(
        (await client.queries.find({ id: "snapshot-read" }, { store: false }))
          .entry.text,
        "fresh",
      );
      assert.equal(
        h.queries.filter((x) => x === "snapshot-read").length,
        2,
        "every Query must invoke its handler",
      );
      assert.equal(
        (await client.fetch.entry({ id: "snapshot-read" }, { store: false }))
          .text,
        "fresh",
      );
      assert.equal(
        await client.models.entry.get({ id: "snapshot-read" }),
        null,
      );
      assert.equal(
        (await client.queries.find({ id: "snapshot-read" })).entry.text,
        "fresh",
      );
      await client.models.entry.delete({ id: "snapshot-read" });
      assert.equal(
        await client.models.entry.get({ id: "snapshot-read" }),
        null,
      );
      assert.equal(
        (await client.queries.find({ id: "snapshot-read" })).entry.text,
        "fresh",
      );
      assert.equal(
        (await client.models.entry.get({ id: "snapshot-read" })).text,
        "fresh",
      );
      const memberships = (
        await h.pool.query(
          "SELECT s.* FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id WHERE r.identity->>'id'='snapshot-read'",
        )
      ).rows;
      assert.deepEqual(memberships, [], "Query/Fetch never enroll implicitly");
      assert.equal((await client.queries.find({ id: "missing" })).entry, null);
      await assert.rejects(
        client.queries.find({ id: "refused-query" }),
        (error) =>
          error.code === "find.denied" && error.execution === "rejected",
      );
      assert.equal(await client.fetch.entry({ id: "missing" }), null);
      await assert.rejects(
        client.queries.find({ id: "snapshot-read" }, { once: true }),
        /option|once/,
      );
      await assert.rejects(
        GeneratedClient.open({
          path: join(dir, "identity"),
          stream: "User:reader",
          connection: {
            url: h.url,
            token: "reader",
            identity: { viewer: "reader" },
          },
        }),
        /identity/,
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A10/A11/A12: close fences an unfinished Query response before any cache commit", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-close-read-"));
    let client;
    try {
      await h.pool.query(
        "INSERT INTO sdk05_entry VALUES('close-read','snapshot','close')",
      );
      const path = join(dir, "db");
      client = await open(path, h.url, "close");
      await client.bootstrap();
      h.holdReads();
      const pending = client.queries.find({ id: "close-read" });
      const rejected = assert.rejects(
        pending,
        (error) => error.code === "action.unavailable",
      );
      await until(
        () => h.heldReadCount === 1,
        "Query response held after server read",
      );
      await Promise.race([
        client.close(),
        new Promise((_, reject) =>
          setTimeout(
            () => reject(Error("close waited for held Query response")),
            1000,
          ),
        ),
      ]);
      await rejected;
      h.releaseReads();
      client = await open(path, undefined, "close");
      assert.equal(
        await client.models.entry.get({ id: "close-read" }),
        null,
        "late closed-connection result wrote reopened Store",
      );
    } finally {
      h.releaseReads();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A5/A6/A7/A11: actual multi-part same-cursor Bootstrap commits one complete unit", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-fragments-"));
    let client, stop;
    try {
      await h.backend.transaction(async ({ tx, streams }) => {
        await tx.query(
          "INSERT INTO sdk05_entry SELECT 'fragment-'||n,'payload-'||n,'fragments' FROM generate_series(1,1001) n",
        );
        streams(["User:fragments"]).track.entry(
          Array.from({ length: 1001 }, (_, i) => "fragment-" + (i + 1)),
        );
      });
      client = await open(join(dir, "db"), undefined, "fragments");
      const sizes = [];
      stop = client.watchSql('SELECT COUNT(*) AS n FROM "Entry"', [], (rows) =>
        sizes.push(rows[0].n),
      );
      await client.connect({ url: h.url, token: "fragments" });
      await client.bootstrap();
      await until(
        async () =>
          (await client.readSql('SELECT COUNT(*) AS n FROM "Entry"'))[0].n ===
          1001,
        "complete Bootstrap unit committed",
      );
      assert.ok(sizes.includes(1001));
      assert.ok(
        sizes.every((size) => size === 0 || size === 1001),
        "fragment published partial Model state",
      );
      const pulls = h.requests
        .filter((request) => request.route === "/sync/pull")
        .map((request) => JSON.parse(request.body));
      assert.ok(
        pulls.length >= 1 &&
          pulls.every((request) => !("continuation" in request)),
        "cursor pages must not request persisted response fragments",
      );
      const state = (
        await client.readSql(
          "SELECT start_cursor,bootstrap_cursor FROM axton_store",
        )
      )[0];
      assert.equal(state.start_cursor, 1);
      assert.equal(state.bootstrap_cursor, 1);
    } finally {
      stop?.();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A10/A11/A12: parked storing Fetch before initial Stream start closes with its retained failure code", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-parked-fetch-"));
    let client, connection;
    try {
      h.holdHandshake();
      client = await open(join(dir, "db"), undefined, "parked-fetch");
      connection = await client.connect({ url: h.url, token: "parked-fetch" });
      await until(
        () => h.requests.some((request) => request.route === "/sync/handshake"),
        "initial handshake held",
      );
      const failure = assert.rejects(
        client.fetch.entry({ id: "not-held" }),
        (error) =>
          error.code === "fetch.unavailable" && error.execution === "unknown",
      );
      assert.equal(
        await client.fetch.entry({ id: "not-held" }, { store: false }),
        null,
      );
      // Local work proceeds while the Fetch is parked: an offline Mutation.
      await publish(client, "local", "independent");
      assert.equal(
        (await client.models.entry.get({ id: "local" })).text,
        "independent",
      );
      assert.equal(
        h.requests.filter(
          (request) =>
            request.route === "/sync/fetch" && JSON.parse(request.body).store,
        ).length,
        0,
      );
      await connection.close();
      await failure;
    } finally {
      h.releaseHandshake();
      await connection?.close();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A5/A9: first storing read waits for durable Stream start while store false remains independent", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-first-read-"));
    let client;
    try {
      await h.backend.transaction(async ({ tx, streams }) => {
        await tx.query(
          "INSERT INTO sdk05_entry VALUES('first-read','A','first')",
        );
        streams(["User:first"]).track.snapshot("first-read");
      });
      h.holdHandshake();
      client = await open(join(dir, "first"), undefined, "first");
      await client.connect({ url: h.url, token: "first" });
      await until(
        () => h.requests.some((request) => request.route === "/sync/handshake"),
        "initial handshake request waiting",
      );
      const storing = client.queries.peek({ id: "first-read" });
      assert.equal(
        (await client.queries.peek({ id: "first-read" }, { store: false }))
          .entry.text,
        "A",
      );
      assert.equal(
        h.requests.filter(
          (request) =>
            request.route === "/sync/actions" &&
            JSON.parse(request.body).store === true,
        ).length,
        0,
        "storing Query must not read before initial S/C",
      );
      assert.equal(
        await client.models.snapshot.get({ id: "first-read" }),
        null,
        "non-storing read wrote cache",
      );
      await h.backend.transaction(async ({ tx, invalidate }) => {
        await tx.query("UPDATE sdk05_entry SET text='B' WHERE id='first-read'");
        invalidate.snapshot("first-read");
      });
      h.releaseHandshake();
      assert.equal((await storing).entry.text, "B");
      assert.equal(
        (await client.models.snapshot.get({ id: "first-read" })).text,
        "B",
      );
    } finally {
      h.releaseHandshake();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A9: authoritative deletion wins over a genuinely earlier Query response", async () => {
    const h = await host(),
      dir = await mkdtemp(join(tmpdir(), "axton-sdk05-stale-"));
    let client;
    try {
      client = await open(join(dir, "db"), h.url, "stale");
      await client.bootstrap();
      const call = await publish(client, "stale-entry", "before");
      assert.equal((await call.wait()).error, null);
      h.holdReads();
      const delayed = client.queries.find({ id: "stale-entry" });
      await until(
        () => h.heldReadCount === 1,
        "server completed old Query snapshot",
      );
      await h.backend.transaction(async ({ tx, invalidate }) => {
        await tx.query("DELETE FROM sdk05_entry WHERE id='stale-entry'");
        invalidate.entry("stale-entry");
      });
      await until(
        async () =>
          (await client.models.entry.get({ id: "stale-entry" })) === null,
        "current authoritative absence",
      );
      h.releaseReads();
      assert.equal(
        (await delayed).entry.text,
        "before",
        "caller still receives its own earlier snapshot",
      );
      assert.equal(
        await client.models.entry.get({ id: "stale-entry" }),
        null,
        "ordinary stale snapshot resurrected current authority",
      );
    } finally {
      h.releaseReads();
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A9/A12: generated Query handler explicitly tracks through its authenticated Stream", async () => {
    const h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-query-track-"));
    let client;
    try {
      await h.backend.transaction(async ({ tx }) => {
        await tx.query(
          "INSERT INTO sdk05_entry VALUES('explicit-track-query','tracked deliberately','query-track')",
        );
      });
      client = await open(join(dir, "db"), h.url, "query-track");
      await client.bootstrap();
      const result = await client.queries.find({ id: "explicit-track-query" });
      assert.equal(result.entry.text, "tracked deliberately");
      const rows = (
        await h.pool.query(
          "SELECT s.stream FROM axton_stream_record s JOIN axton_record r ON r.id=s.record_id WHERE r.identity->>'id'='explicit-track-query'",
        )
      ).rows;
      assert.deepEqual(rows, [{ stream: "User:query-track" }]);
      await h.backend.transaction(async ({ tx, invalidate }) => {
        await tx.query(
          "UPDATE sdk05_entry SET text='later authority' WHERE id='explicit-track-query'",
        );
        invalidate.entry("explicit-track-query");
      });
      await until(
        async () =>
          (await client.models.entry.get({ id: "explicit-track-query" }))
            ?.text === "later authority",
        "explicit Query holding receives later authority",
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A14: a changed schema upgrades at open, keeps every row and Bootstraps the new selection over handshake and pull alone", async () => {
    let h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-schema-"));
    let client;
    try {
      await h.backend.transaction(async ({ tx, streams }) => {
        await tx.query(
          "INSERT INTO sdk05_entry VALUES('schema-entry','old entry','schema'),('schema-snapshot','old snapshot','schema')",
        );
        streams(["User:schema"]).track.entry("schema-entry");
        streams(["User:schema"]).track.snapshot("schema-snapshot");
      });
      const path = join(dir, "db");
      client = await open(path, h.url, "schema");
      await client.bootstrap();
      assert.equal(
        (await client.queries.peek({ id: "schema-snapshot" })).entry.text,
        "old snapshot",
      );
      const before = (
        await client.readSql(
          "SELECT materialization,start_cursor,bootstrap_cursor,cursor FROM axton_store",
        )
      )[0];
      await client.close();
      client = undefined;
      await h.backend.transaction(async ({ tx, invalidate }) => {
        await tx.query(
          "UPDATE sdk05_entry SET text='new content' WHERE owner='schema'",
        );
        invalidate.entry("schema-entry");
        invalidate.snapshot("schema-snapshot");
      });
      await h.close();
      // The upgraded backend serves only its own materialization.
      h = await host({ upgraded: true });
      client = await UpgradedClient.open({ path, stream: "User:schema" });
      // The upgrade commits at open, with no network: rows stay, B is absent
      // and S = C.
      const opened = (
        await client.readSql(
          "SELECT materialization,start_cursor,bootstrap_cursor,cursor FROM axton_store",
        )
      )[0];
      assert.notEqual(opened.materialization, before.materialization);
      assert.deepEqual(
        [opened.start_cursor, opened.bootstrap_cursor, opened.cursor],
        [before.cursor, null, before.cursor],
      );
      assert.equal(
        (await client.models.snapshot.get({ id: "schema-snapshot" })).text,
        "old snapshot",
      );
      await client.connect({ url: h.url, token: "schema" });
      await client.bootstrap();
      // Changes published after C arrive by Sync from C.
      await until(
        async () =>
          (await client.models.entry.get({ id: "schema-entry" })).text ===
            "new content" &&
          (await client.models.snapshot.get({ id: "schema-snapshot" }))
            .text === "new content",
        "Sync from C delivers the changes made after it",
      );
      const after = (
        await client.readSql(
          "SELECT start_cursor,bootstrap_cursor FROM axton_store",
        )
      )[0];
      assert.equal(after.bootstrap_cursor, after.start_cursor);
      assert.deepEqual(
        h.requests
          .map((request) => request.route)
          .filter((route) => !["/sync/handshake", "/sync/pull"].includes(route)),
        [],
        "the upgrade reads only handshake and Stream ranges",
      );
      assert.deepEqual(
        h.responses.filter((response) => response.status === 409),
        [],
        "no request ran in the previous context",
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A14: an upgrade to Model version 2 Bootstraps its new field over the held row at the same cursor", async () => {
    let h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-versioned-"));
    let client;
    try {
      await h.backend.transaction(async ({ tx, streams }) => {
        await tx.query(
          "INSERT INTO sdk05_entry VALUES('versioned-entry','same row','versioned')",
        );
        streams(["User:versioned"]).track.entry("versioned-entry");
      });
      const path = join(dir, "db");
      client = await open(path, h.url, "versioned");
      await client.bootstrap();
      const before = (
        await client.readSql(
          "SELECT materialization,bootstrap_cursor,cursor FROM axton_store",
        )
      )[0];
      const evidence = JSON.parse(
        (
          await client.readSql(
            "SELECT evidence FROM axton_authority WHERE model='Entry'",
          )
        )[0].evidence,
      );
      assert.equal(
        (await client.models.entry.get({ id: "versioned-entry" })).text,
        "same row",
      );
      await client.close();
      client = undefined;
      await h.close();
      h = await host({ versioned: true });
      client = await VersionedClient.open({
        path,
        stream: "User:versioned",
        connection: { url: h.url, token: "versioned" },
      });
      assert.notEqual(
        (await client.readSql("SELECT materialization FROM axton_store"))[0]
          .materialization,
        before.materialization,
      );
      await client.bootstrap();
      assert.deepEqual(
        h.requests
          .map((request) => request.route)
          .filter((route) => !["/sync/handshake", "/sync/pull"].includes(route)),
        [],
        "the upgrade reads only handshake and Stream ranges",
      );
      const row = await client.models.entry.get({ id: "versioned-entry" });
      assert.equal(row.text, "same row");
      assert.equal(row.note, "new schema");
      const after = JSON.parse(
        (
          await client.readSql(
            "SELECT evidence FROM axton_authority WHERE model='Entry'",
          )
        )[0].evidence,
      );
      assert.equal(
        after.membership.cursor,
        evidence.membership.cursor,
        "Bootstrap restamped an unchanged record",
      );
      assert.equal(
        (await client.readSql("SELECT bootstrap_cursor FROM axton_store"))[0]
          .bootstrap_cursor,
        before.bootstrap_cursor,
        "Bootstrap completes at S = C, the unchanged Stream head",
      );
      assert.equal(
        (
          await h.pool.query(
            "SELECT head FROM axton_stream WHERE stream='User:versioned'",
          )
        ).rows[0].head,
        String(before.cursor),
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A14: a fresh file's queued Mutations upgrade at the first handshake to generation 2 and push in its context", async () => {
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-generation-")),
      path = join(dir, "db");
    let client = await open(path),
      h;
    try {
      await publish(client, "generation-queued", " queued ");
      const offline = (
        await client.readSql("SELECT materialization FROM axton_store")
      )[0].materialization;
      await client.close();
      h = await host({ projectionGeneration: "2" });
      client = await open(path, h.url);
      await until(
        async () => (await client.syncState()).pending === 0,
        "queued Mutation settles after the upgrade",
      );
      const store = (
        await client.readSql("SELECT materialization FROM axton_store")
      )[0];
      assert.notEqual(store.materialization, offline);
      assert.equal(h.batches.length, 1);
      assert.equal(JSON.parse(h.batches[0]).materialization, store.materialization);
      assert.equal(
        (await client.models.entry.get({ id: "generation-queued" })).text,
        "queued",
      );
    } finally {
      await client?.close();
      await h?.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A14: a backend projection change upgrades an initialized Store with no request in the dropped context", async () => {
    let h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-projection-")),
      path = join(dir, "db");
    let client;
    try {
      await h.backend.transaction(async ({ tx, streams }) => {
        await tx.query(
          "INSERT INTO sdk05_entry VALUES('projection-entry','before','projection')",
        );
        streams(["User:projection"]).track.entry("projection-entry");
      });
      client = await open(path, h.url, "projection");
      await client.bootstrap();
      const before = (
        await client.readSql("SELECT materialization FROM axton_store")
      )[0].materialization;
      await client.close();
      client = undefined;
      await h.close();
      // The previous materialization is not retained: any request in the old
      // context would be refused with 409 context_mismatch.
      h = await host({ projectionGeneration: "2" });
      client = await open(path, h.url, "projection");
      await until(async () => {
        const row = (
          await client.readSql(
            "SELECT materialization,start_cursor FROM axton_store",
          )
        )[0];
        return row.materialization !== before && row.start_cursor !== null;
      }, "handshake-reported generation upgrades the Store");
      await client.bootstrap();
      assert.equal(
        (await client.models.entry.get({ id: "projection-entry" })).text,
        "before",
      );
      assert.deepEqual(
        h.responses.filter((response) => response.status === 409),
        [],
        "no request ran in the dropped context",
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A14: a Batch frozen before an upgrade is sent after it with the same digest in the new context and accepted", async () => {
    let h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-frozen-")),
      path = join(dir, "db");
    let client;
    try {
      client = await open(path, h.url, "frozen");
      await client.bootstrap();
      h.holdMutations();
      await publish(client, "frozen-entry", " frozen ", "private");
      await until(() => h.batches.length > 0, "Batch frozen and sent");
      const before = JSON.parse(h.batches[0]);
      await client.close();
      client = undefined;
      await h.close();
      h = await host({ upgraded: true });
      client = await UpgradedClient.open({
        path,
        stream: "User:frozen",
        connection: { url: h.url, token: "frozen" },
      });
      await until(
        async () => (await client.syncState()).pending === 0,
        "the frozen Batch settles after the upgrade",
      );
      const resent = JSON.parse(h.batches[0]);
      assert.equal(resent.digest, before.digest);
      assert.equal(resent.batchId, before.batchId);
      assert.notEqual(resent.materialization, before.materialization);
      assert.equal(
        resent.materialization,
        (await client.readSql("SELECT materialization FROM axton_store"))[0]
          .materialization,
      );
      assert.deepEqual(h.executions, ["frozen-entry"]);
      // Executed in the new contract: its receipt snapshot installs.
      assert.equal(
        (await client.models.entry.get({ id: "frozen-entry" })).text,
        "frozen",
      );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A14: a Batch executed before an upgrade whose receipt was lost replays its old label and settles without installing its snapshot", async () => {
    let h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-lost-")),
      path = join(dir, "db");
    let client;
    try {
      client = await open(path, h.url, "lost");
      await client.bootstrap();
      h.loseNext(1000);
      await publish(client, "lost-entry", " lost ", "private");
      await until(() => h.executions.includes("lost-entry"), "Batch executed");
      const before = JSON.parse(h.batches[0]);
      await client.close();
      client = undefined;
      await h.close();
      h = await host({ upgraded: true });
      client = await UpgradedClient.open({
        path,
        stream: "User:lost",
        connection: { url: h.url, token: "lost" },
      });
      await until(
        async () => (await client.syncState()).pending === 0,
        "the replayed receipt settles after the upgrade",
      );
      const resent = JSON.parse(h.batches[0]);
      assert.equal(resent.digest, before.digest);
      assert.notEqual(resent.materialization, before.materialization);
      assert.deepEqual(h.executions, [], "the resend replays, it does not execute");
      const receipt = JSON.parse(
        h.responses.find((response) => response.route === "/sync/mutations").body,
      );
      assert.equal(receipt.results[0].outcome.kind, "accepted");
      assert.equal(receipt.results[0].outcome.materialization, before.materialization);
      // The snapshot was loaded in the previous contract and is not installed.
      assert.equal(await client.models.entry.get({ id: "lost-entry" }), null);
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("60 concurrent Mutations from one Store settle over push and pull alone, with under 10 serialization retries", async () => {
    const h = await host();
    const dir = await mkdtemp(join(tmpdir(), "axton-sdk05-concurrent-")),
      path = join(dir, "db");
    let client;
    try {
      client = await open(path, h.url, "concurrent");
      await client.bootstrap();
      const requests = h.requests.length,
        transactions = h.transactions.length;
      const calls = await Promise.all(
        Array.from({ length: 60 }, (_, i) =>
          publish(client, `concurrent-${i}`, ` reply ${i} `),
        ),
      );
      const outcomes = await Promise.all(calls.map((call) => call.wait()));
      assert.deepEqual(
        outcomes.filter((outcome) => outcome.error !== null),
        [],
      );
      assert.equal((await client.syncState()).pending, 0);
      for (let i = 0; i < 60; i++)
        assert.equal(
          (await client.models.entry.get({ id: `concurrent-${i}` })).text,
          `reply ${i}`,
        );
      const routes = new Set(h.requests.slice(requests).map((r) => r.route));
      assert.deepEqual(
        [...routes].filter((route) => !["/sync/mutations", "/sync/pull"].includes(route)),
        [],
        "settlement sends nothing beyond push and pull",
      );
      const observed = h.transactions.slice(transactions);
      const retries = observed.reduce((n, tx) => n + tx.attempts - 1, 0);
      assert.ok(retries < 10, `${retries} serialization retries`);
      assert.equal(observed.filter((tx) => tx.outcome === "escaped").length, 0);
      // Only the served routes exist; any other sync path is not found.
      for (const route of ["/sync/load", "/sync/push", "/sync/unknown"])
        assert.equal(
          (await fetch(h.url + route, { method: "POST", body: "{}" })).status,
          404,
          route,
        );
    } finally {
      await client?.close();
      await h.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
  test("A11/A12: generated Dart crosses HTTP/WebSocket, SQLite and PostgreSQL", async () => {
    assert.ok(process.env.AXTON_DART, "Dart must participate in the host gate");
    const h = await host();
    try {
      const result = await promisify(execFile)(
        process.env.AXTON_DART,
        ["run", "client.dart", h.url],
        { cwd: new URL(".", import.meta.url).pathname, timeout: 60000 },
      );
      assert.match(result.stdout, /Dart protocol5 actual host: PASS/);
    } finally {
      await h.close();
    }
  });
}
