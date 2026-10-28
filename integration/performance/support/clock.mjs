import assert from 'node:assert/strict';

export const runnerClockDomain = `node:${process.pid}`;
export const nowUs = () => Number(process.hrtime.bigint() / 1000n);

/** Parent side of the sample's pre/post measurement handshake. */
export class ClockHandshake {
  endpoints = {};
  #round;
  constructor(write) { this.write = write; }
  #probe() {
    const id = this.#round.probes.length;
    this.#round.pending = {id, localSendUs: nowUs()};
    this.write(JSON.stringify({type: 'clock.probe', id}) + '\n');
  }
  accept(event, receivedAtUs = nowUs()) {
    if (event.phase === 'clock.ready') {
      assert.ok(!this.#round, 'overlapping clock calibration');
      this.#round = {phase: event.calibrationPhase, clockDomain: event.clockDomain, probes: []};
      this.#probe();
    } else if (event.phase === 'clock.reply') {
      const round = this.#round;
      assert.ok(round && round.pending.id === event.id && round.phase === event.calibrationPhase, 'clock reply correlation');
      round.probes.push({...round.pending, remoteReceiveUs: event.remoteReceiveUs,
        remoteSendUs: event.remoteSendUs, localReceiveUs: receivedAtUs});
      if (round.probes.length < 8) this.#probe();
      else {
        this.endpoints[round.phase] = {clockDomain: round.clockDomain, probes: round.probes};
        this.#round = undefined;
        this.write(JSON.stringify({type: 'clock.done'}) + '\n');
      }
    } else return false;
    return true;
  }
}

// The remote receive/send occurred between local send/receive. No symmetry or
// one-way network-latency estimate is needed for these offset bounds.
export function probeBounds(probe) {
  const {localSendUs, remoteReceiveUs, remoteSendUs, localReceiveUs} = probe;
  assert.ok([localSendUs, remoteReceiveUs, remoteSendUs, localReceiveUs].every(Number.isFinite), 'finite clock probe');
  assert.ok(localReceiveUs >= localSendUs && remoteSendUs >= remoteReceiveUs, 'monotonic clock probe');
  const lowerUs = localSendUs - remoteReceiveUs - 1;
  const upperUs = localReceiveUs - remoteSendUs + 1;
  assert.ok(upperUs >= lowerUs, 'clock probe has impossible elapsed times');
  return {lowerUs, upperUs};
}

export function calibration(clockDomain, referenceClockDomain, probes, method) {
  assert.ok(probes.length, 'clock calibration needs probes');
  const bounds = probes.map(probeBounds);
  const offsetLowerUs = Math.max(...bounds.map(b => b.lowerUs));
  const offsetUpperUs = Math.min(...bounds.map(b => b.upperUs));
  assert.ok(offsetUpperUs >= offsetLowerUs, 'clock probes disagree; preserve raw clocks instead of inventing alignment');
  return {clockDomain, referenceClockDomain, offsetLowerUs, offsetUpperUs, method, probes};
}

export function sampleNativeClock(nativeClock, count = 8) {
  return Array.from({length: count}, () => {
    const localSendUs = nowUs(), remoteReceiveUs = nativeClock(), localReceiveUs = nowUs();
    return {localSendUs, remoteReceiveUs, remoteSendUs: remoteReceiveUs, localReceiveUs};
  });
}

// Require endpoint intervals to agree with one constant offset. This is a
// bounded alignment, not a claim that differently implemented clocks cannot drift.
export function combineCalibrations(start, end) {
  assert.equal(start.clockDomain, end.clockDomain);
  assert.equal(start.referenceClockDomain, end.referenceClockDomain);
  const offsetLowerUs = Math.max(start.offsetLowerUs, end.offsetLowerUs);
  const offsetUpperUs = Math.min(start.offsetUpperUs, end.offsetUpperUs);
  assert.ok(offsetUpperUs >= offsetLowerUs, 'clock offset changed outside measured bounds; common-axis trace unavailable');
  return {...start, offsetLowerUs, offsetUpperUs, endpoints: {start, end},
    model: 'constant offset; validated against before/after calibration intervals',
    observedMidpointShiftUs: ((end.offsetLowerUs + end.offsetUpperUs) - (start.offsetLowerUs + start.offsetUpperUs)) / 2};
}

export function clockRecord(edge, source = 'benchmark') {
  return {schemaVersion: 1, kind: 'clock', name: 'clock.origin', source,
    clockDomain: edge.clockDomain, startUs: 0, attributes: edge};
}

export function retainClockRecords(records) {
  const clocks = records.filter(record => record.kind === 'clock');
  records.length = 0;
  records.push(...clocks);
}
