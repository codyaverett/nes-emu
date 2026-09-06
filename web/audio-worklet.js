// AudioWorklet processor for the NES page (issue #50).
//
// The main thread posts Float32Array chunks of 44.1 kHz mono samples
// from Emulator.take_audio(); this processor keeps them in a ring buffer
// and plays them back. On underrun it holds the last sample (the same
// rule as the SDL callback in src/main.rs) and counts the event. Every
// REPORT_EVERY process() calls it posts {consumed, underruns} so the
// main thread can estimate how many samples are still queued.
const RING_SIZE = 1 << 15; // 32768 samples, about 740 ms; well above target
const REPORT_EVERY = 8; // 8 x 128 frames = about 23 ms

class NesAudioProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.ring = new Float32Array(RING_SIZE);
    this.readPos = 0;
    this.writePos = 0;
    this.queued = 0;
    this.last = 0;
    this.consumed = 0;
    this.underruns = 0;
    this.dropped = 0;
    this.calls = 0;
    // Underruns only count once samples have arrived since the last
    // clear, so a paused or not-yet-started game is not a starved one.
    this.armed = false;
    this.port.onmessage = (event) => {
      const data = event.data;
      if (data instanceof Float32Array) {
        this.push(data);
      } else if (data && data.type === "clear") {
        this.readPos = this.writePos = this.queued = 0;
        this.last = 0;
        this.armed = false;
      }
    };
  }

  push(samples) {
    this.armed = true;
    for (let i = 0; i < samples.length; i++) {
      if (this.queued >= RING_SIZE) {
        this.dropped++;
        continue;
      }
      this.ring[this.writePos] = samples[i];
      this.writePos = (this.writePos + 1) % RING_SIZE;
      this.queued++;
    }
  }

  process(inputs, outputs) {
    const out = outputs[0];
    const channel = out[0];
    let underran = false;
    for (let i = 0; i < channel.length; i++) {
      if (this.queued > 0) {
        this.last = this.ring[this.readPos];
        this.readPos = (this.readPos + 1) % RING_SIZE;
        this.queued--;
        this.consumed++;
      } else {
        underran = true;
      }
      channel[i] = this.last;
    }
    for (let c = 1; c < out.length; c++) out[c].set(channel);
    if (underran && this.armed) this.underruns++;
    if (++this.calls % REPORT_EVERY === 0) {
      this.port.postMessage({
        consumed: this.consumed,
        queued: this.queued,
        underruns: this.underruns,
        dropped: this.dropped,
      });
    }
    return true;
  }
}

registerProcessor("nes-audio", NesAudioProcessor);
