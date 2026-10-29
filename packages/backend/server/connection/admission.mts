import type { IncomingMessage } from "node:http";
import type { JsonValue } from "../bindings/host-contract.mts";

export type Authenticate = (
  request: IncomingMessage,
) => Promise<string | null | undefined> | string | null | undefined;
/**
 * An admission refusal: the HTTP status and JSON body the listener answers,
 * marked `axton-admission: refused` so a client stops its connection instead
 * of retrying. `status` is 400..599.
 */
export type AdmissionRefusal = { status: number; body: JsonValue };
/**
 * Decides, before any route runs, whether a client may use the listener: its
 * request (the headers the client was configured with included) and the user
 * id `authenticate` resolved, or `null` without one. Answer `null` or
 * `undefined` to admit, or the refusal to answer instead.
 */
export type Admit = (
  request: IncomingMessage,
  userId: string | null,
) =>
  | Promise<AdmissionRefusal | null | undefined>
  | AdmissionRefusal
  | null
  | undefined;
/** Development only: the bearer token is used verbatim as the user id. Never use in production. */
export function devAuth(): Authenticate {
  return (request) => {
    const header = request.headers.authorization;
    if (typeof header !== "string" || !header.startsWith("Bearer "))
      return null;
    const id = header.slice("Bearer ".length).trim();
    return id === "" ? null : id;
  };
}
/** The response header that marks an admission refusal. */
export const ADMISSION_HEADER = "axton-admission";
/** An admission refusal ready to answer: its status and JSON text. */
export type Refusal = { status: number; body: string };
/**
 * What `admit` answered, checked: `null` admits, a refusal needs a status of
 * 400..599 and a body JSON can encode. Anything else is a defect of the hook.
 */
function checkedRefusal(answer: unknown): Refusal | null {
  if (answer === null || answer === undefined) return null;
  const { status, body } = answer as Partial<AdmissionRefusal>;
  const text = body === undefined ? undefined : JSON.stringify(body);
  if (
    !Number.isInteger(status) ||
    (status as number) < 400 ||
    (status as number) > 599 ||
    typeof text !== "string"
  )
    throw new TypeError(
      "admit must answer null or {status: 400..599, body: JSON}",
    );
  return { status: status as number, body: text };
}
/** Authenticate a request, then ask the application's `admit` about it. */
export type Admission = (
  request: IncomingMessage,
) => Promise<{ owner: string | null; refusal: Refusal | null }>;

/** Shared admission for HTTP routes and WebSocket upgrades. */
export function createAdmission(options: {
  authenticate: Authenticate;
  admit?: Admit | undefined;
}): Admission {
  const authenticate = async (request: IncomingMessage) => {
    const id = await options.authenticate(request);
    if (typeof id !== "string") return null;
    const trimmed = id.trim();
    return trimmed === "" ? null : trimmed;
  };
  const admit = async (
    request: IncomingMessage,
  ): Promise<{ owner: string | null; refusal: Refusal | null }> => {
    const owner = await authenticate(request);
    if (!options.admit) return { owner, refusal: null };
    return {
      owner,
      refusal: checkedRefusal(await options.admit(request, owner)),
    };
  };
  return admit;
}
