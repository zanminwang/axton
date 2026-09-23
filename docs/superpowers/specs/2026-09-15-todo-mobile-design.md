# Collaborative mobile To-do demo

Status: implementation specification; the mobile application and React Native integration are not implemented by this document.

Tracking: [mobile #31](https://github.com/zanminwang/axton/issues/31), [browser dependencies #59](https://github.com/zanminwang/axton/issues/59), [web demo #72](https://github.com/zanminwang/axton/issues/72).

## 1. Goal and scope

**Delivery order:** [React Native support #100](https://github.com/zanminwang/axton/issues/100) blocks [demo #31](https://github.com/zanminwang/axton/issues/31). #100 owns native carrier, SDK/transport adaptation, and independent iOS runtime validation. #31 consumes that supported integration and owns the To-do schema/backend/UI, example migration, and application-level demonstration.

Replace `examples/rust-round-trip` with a small, understandable collaborative To-do example at `examples/todo`. A developer should be able to understand the schema, run two independent phones, and observe local writes, durable offline work, and synchronization through an application-owned backend.

The user selected React Native with TypeScript and reduced the application to **Add task and Done**. Two phones show the same shared list. Their headers identify different signed-in demo participants with avatars. The two-card presentation belongs to the demonstration; an installed app renders one screen.

Excluded: assignment, replies, reply counts, title editing, task deletion, list management, filters, counters, dashboards, invitations, account registration, avatar upload, desktop clients, browser implementation, and video production. No additional task controls or promotional copy in the screen.

## 2. Implementation defaults

These complete the agreed minimal scope without adding product features:

- iOS simulators are the first required runtime target. Android and physical-device support require separate evidence before being claimed.
- Use an Expo development build and an iOS Expo native module. Expo Go cannot include AXTON's custom native library. See [Expo development builds](https://docs.expo.dev/develop/development-builds/introduction/) and [local native modules](https://docs.expo.dev/workflow/customizing/).
- Use the existing Rust client engine and its SQLite storage, plus a TypeScript/Node backend with Prisma and PostgreSQL.
- Seed two demo identities, `alice` / Alice and `bob` / Bob. Select the identity through launch configuration, not a new login flow. This is a local development identity mechanism, not production authentication.
- Derive a circular avatar from the participant's initial and a fixed color. No avatar column or network image dependency.
- Keep exactly two business models: `User` and `Todo`. Retain the creator relation to make the schema useful to explain.
- A completed task stays in place; its checkbox can set it back to incomplete. Order rows by `id` ascending for deterministic presentation. Do not add a timestamp just for sorting.

## 3. Data model

### Business tables

| Model | Field | Type | Meaning |
| --- | --- | --- | --- |
| User | id | String, primary key | Stable demo user identity |
| User | name | String | Name in the phone header |
| Todo | id | String, primary key | UUID string generated once on the creating device |
| Todo | title | String | Task text |
| Todo | done | Boolean | Completion state; explicitly false on creation |
| Todo | createdById | String, foreign key to User.id | Creator; never an assignee |

`Todo.createdBy` is a generated relation, not another stored column. User records are seeded by the backend; the app cannot create or edit them. There is no `List`, `Membership`, `Reply`, or `Assignment` table. All demo participants share the fixed channel `todo:demo`.

Target AXTON schema:

```text
model User {
  id String
  name String
  @@id(id)
}

model Todo {
  id String
  title String
  done Bool
  createdById String
  createdBy User @reference(via: [createdById])
  @@id(id)
}

mutation AddTodo { todo Todo.create }
mutation SetTodoDone { todo Todo.update<done> }
```

Compile this with the repository compiler and use generated builders. Do not hand-maintain a second schema or generated interfaces. The default lifecycle dependency of an update on its pending create must be verified with an offline add-then-done test; do not add ordering annotations speculatively.

The backend has these two business tables plus AXTON's existing persistence tables. Each phone has an independent SQLite database containing its local business data and engine-owned queue, cursor, identity, and other sync metadata. Those internal tables are not application models. The developer walkthrough must make that distinction.

### Operations and backend rules

| Operation | Input | Local effect | Backend rule |
| --- | --- | --- | --- |
| AddTodo | id, title, done=false, createdById | Insert a task and durably queue its creation | Trim title; reject empty trimmed text; require creator to equal authenticated user; require done=false; insert once and notify `todo:demo` |
| SetTodoDone | id, done | Set the checkbox and durably queue the requested value | Require the task to exist; either demo participant can set done; update and notify `todo:demo` |

Use `MutationRejected` codes `todo.title_empty`, `todo.creator_invalid`, `todo.initial_state_invalid`, `todo.missing`, and `todo.id_conflict` for these expected business refusals. A completion request without a boolean `done` never reaches the handler: the runtime refuses it as `mutation.invalid`. Unexpected database failures remain retryable server failures. Never convert an arbitrary Prisma failure into a successful operation.

The UI trims text and prevents empty submission; the server repeats validation. Repeating a frozen request uses AXTON's durable receipt contract. A genuinely different create with an existing ID is rejected; it must not overwrite that task. `createdById` is immutable after creation.

Authenticate only the two demo identities. Loaders allow those identities to read both users and all tasks on `todo:demo`; reject or return no data for other scopes according to the existing server API. Publish seeds and handler changes inside their database transactions using the existing notification/persistence contract.

## 4. Screen behavior

```text
┌───────────────────────┐  ┌───────────────────────┐
│ (A) Alice             │  │ (B) Bob               │
│                       │  │                       │
│ To-do                 │  │ To-do                 │
│ □ Buy milk            │  │ □ Buy milk            │
│ □ Book a table        │  │ □ Book a table        │
│ ☑ Pick up keys        │  │ ☑ Pick up keys        │
│                       │  │                       │
│ Add task…         +   │  │ Add task…         +   │
└───────────────────────┘  └───────────────────────┘
```

- Use native system typography, a neutral background, thin separators, and comfortable touch targets. Keep completed rows readable with a muted title and strike-through.
- Keep the add field visible when the list is empty. Return on the keyboard and the plus button submit the same action. Prevent duplicate submission while that local write is committing; clear the field only after local commit succeeds.
- A checkbox is accessible as a checkbox with its task title and checked state. Keep keyboard focus usable after adding. Long titles wrap; the list scrolls above the keyboard.
- Subscribe and watch local records. React state holds input and transient presentation state, not a second authoritative task store. A local committed write is visible without waiting for the network.
- Initial startup with no cached data shows a short loading state. A first-ever launch offline explains that the initial list needs a connection. After initial loading, offline startup shows persisted data immediately.
- No always-visible sync dashboard. Show a short conditional message for an actual local write failure or server rejection. A transient connection failure must not block local work or imply that a committed offline change was lost. Clear obsolete error feedback after recovery; inspect rejections as well as transport errors.
- Keep deliberate network disruption, reset, and diagnostic controls in launch/test tooling. A hidden diagnostic view may be used by the runner, but is not the ordinary screen or marketing footage.

## 5. Architecture and ownership

The platform requirements below belong to #100 and describe what #31 consumes. They are not SDK implementation tasks for the demo agent.

```text
React Native / TypeScript             TypeScript / Node
generated client                     generated backend
        │                                  │
mobile host adapter  ─── HTTP + WS ─── AXTON server
        │                                  │
Expo iOS string bridge                Prisma transaction
        │                                  │
Rust RuntimeHost + SQLite             PostgreSQL
```

The existing [JS client](../../../packages/client-js/index.mts) imports `node:module`, `node:events`, `node:async_hooks`, and `ws`. Installing it in React Native is insufficient. Prerequisite #100 supplies a native carrier and platform adaptation; it must not substitute an in-memory store, REST-only UI, Node shim bundle, or custom synchronization engine.

Use a small native carrier over [RuntimeHost](../../../bindings/common/src/lib.rs). Dispatch SQLite work on a serial background queue; preserve UTF-8, free every Rust-owned returned string exactly once, and convert failures into rejected promises. Open each database in persistent application storage. Do not generate a new client identity on every launch or reuse one identity across independent installations.

Keep the implementation small: first build the React Native adapter needed by this demo. Reuse existing platform-neutral code directly; extract a shared helper only when a concrete Node/mobile use requires it. A generic client factory or broad SDK refactor is not a prerequisite. Keep Node's existing entry point and transaction/savepoint behavior unchanged. React Native uses its own HTTP/WebSocket carrier and transaction adapter. The initial mobile adapter supports the generated read/watch/mutation and transaction surface needed by this example; do not advertise full Node raw-API parity. It need not expose raw nested savepoints. It must still reject operations after transaction completion, drain queued work before rollback, detect unawaited operations, and keep transactions isolated.

Rust owns storage, queueing, receipts, replay, cursors, and connection scheduling. Follow [architecture](../../engineering/architecture.md), [guarantees](../../engineering/guarantees.md), and the [live-session decision](../../engineering/architecture/client/connection/controller/live-session.md). At the planning baseline, the Rust `LiveSession` migration in #58 is a design rather than an available command family. Reuse it if it has landed when implementation begins. Otherwise adapt the existing host orchestration with the smallest necessary platform boundary. Avoid copying a whole SDK or making #31 silently implement all of #58; explain any shared-code extraction in terms of the behavior it enables.

HTTP push and post-subscription HTTP catch-up accompany WebSocket live delivery. Preserve cancellation, stale-session invalidation, bounded buffering, and catch-up on overflow. Do not replace live delivery with periodic polling. Verify authentication on the actual React Native WebSocket implementation.

## 6. Offline and concurrent behavior

1. Alice and Bob load the same seeded users and tasks through separate databases and client identities.
2. Alice adds a task. Her local commit appears immediately; Bob receives it after backend acceptance and delivery.
3. Bob marks it done. Both converge to the authoritative state.
4. Disconnect Alice. She can add and complete a new task while Bob continues online.
5. Terminate and relaunch Alice while disconnected. The task, checkbox, client identity, and queued work survive. The test build must bundle JavaScript so that Metro availability is not confused with data persistence.
6. Reconnect Alice. Queued creation precedes dependent completion, remote changes catch up, and both devices converge without duplicate tasks.

Concurrent `SetTodoDone` operations store explicit booleans, not a server-side toggle. For an existing row, the final value follows the backend's committed update order; no client-wall-clock ordering or merge claim. Test controlled commit order in both directions, and assert convergence to PostgreSQL after pending work settles. Setting true twice remains true. Different task IDs preserve independent additions.

First-time offline bootstrap, background sync while iOS suspends the process, arbitrary crash-boundary recovery, cross-account privacy, and multi-workspace authorization are not claimed by this demo.

## 7. Example migration and developer explanation

The new example owns one schema, backend, seed dataset, and product behavior for mobile and later web. Compile separate generated client entry points for the Node test runner and React Native runtime from that same schema.

Move the legacy Entry/Edit round-trip harness into `integration/e2e/fixtures/round-trip` before removing the old public example. Retain normalization, rejection/rollback, retry, dependent operation, live catch-up, and JS/Dart interoperability assertions. Update relative paths and compiler runtime arguments. Rewrite public quickstart snippets around Todo; retain focused Entry-based API fixtures only where their broader fields are still the subject being documented.

The example README explains prerequisites, backend launch, two simulator identities, reset/seed, and a short developer walkthrough: two tables → two mutations → backend handlers and notification → local watch → real offline/reconnect behavior. Keep video scripts and production notes under `marketing/`; link the example from the existing video briefs without inventing claims.

## 8. Acceptance criteria

| ID | Required evidence |
| --- | --- |
| A1 | Two independently installed iOS simulator apps, different users/databases/client IDs, same list, avatar headers, only Add task / Done controls |
| A2 | Generated User/Todo schema and both mutation builders compile; backend validates creator/title/initial state and publishes transactionally |
| A3 | Add and Done render after local commit without network; both users receive remote changes through the real backend |
| A4 | Offline add-then-done survives process termination/relaunch with bundled JS and the same database/client identity |
| A5 | Reconnect submits pending work, catches up missed changes, drains the queue, and produces identical authoritative task data on both phones and PostgreSQL |
| A6 | Duplicate receipt retry does not duplicate tasks; concurrent completion behavior is verified in both controlled commit orders |
| A7 | Local failure/rejection has concise conditional feedback; empty submission and duplicate local submission are prevented |
| A8 | Legacy regression coverage, Node and Dart consumers, generation scripts, and documentation checks remain working after old example removal |
| A9 | A fresh checkout has documented launch/reset steps and a reproducible two-phone walkthrough; verified revision, toolchain and runtime limits are recorded |

Implementation and validation steps: [plan](../plans/2026-09-15-todo-mobile.md). Agent entry point: [handoff](../handoffs/2026-09-15-todo-mobile.md).
