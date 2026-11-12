import { openStore } from "./store-fixture.mjs";
// Worker body for `worker.test.mjs`: opens a client on `workerData.path`,
// writes inside a transaction whose callback never finishes, and says so.
import { parentPort, workerData } from "node:worker_threads";
import { Client } from "../../../packages/frontend/client-js/index.mts";

const paths = workerData.paths ?? [workerData.path];
let holding = 0;
for (const path of paths) {
  const client = await openStore(Client, { path, schema: workerData.schema });
  void client.transaction(async (tx) => {
    await tx.submitMutation(
      "Create",
      1,
      { entry: { id: "worker", text: "held", note: null } },
      (value) => value,
    );
    if (++holding === paths.length) parentPort.postMessage("holding");
    await new Promise(() => {});
  });
}
