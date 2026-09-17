/**
 * RF-Limiter PLAY surface.
 *
 * The page builds itself from the parameter schema the host sends back, so it
 * has no private list of controls: adding one in the Rust contract makes it
 * appear here. Pages become groups; a float is a knob, a boolean a switch, an
 * enum a row of choices, and a meter a bar updated by RackForge's canonical
 * parameter stream.
 *
 * Three things this surface has to get right to be usable on a stage rather
 * than in a screenshot.
 *
 * **It must not rebuild itself while a hand is on it.** The host sends a fresh
 * context whenever the session revision moves — which includes every write this
 * page makes — and replacing the DOM under a finger loses the gesture. The
 * panel is built once and patched thereafter.
 *
 * **It must not flood the host.** A Rack Slot edits its plugin through an
 * isolated instance: every `set_parameter` opens a plugin, loads state, applies
 * one value and saves it again. Writes are coalesced per parameter and sent one
 * at a time, at most sixteen times a second.
 *
 * **A value being edited belongs to the person editing it.** While a control is
 * held, values arriving from the host for that parameter are ignored, because
 * they are echoes of writes already in flight.
 */
(function () {
  "use strict";

  const PROTOCOL = "rackforge.plugin.web@1";
  const REQUEST_PREFIX = "rf-limiter-";
  const SURFACE_LABEL = "GR";
  /// Milliseconds between flushes of the write queue.
  const WRITE_INTERVAL = 60;
  /// How long a status message stays before the connection line returns.
  const STATUS_LINGER = 4000;
  /// How long to wait for the host before giving up on a request.
  const REQUEST_TIMEOUT = 8000;

  const panelElement = document.getElementById("panel");
  const presetElement = document.getElementById("presets");
  const statusElement = document.getElementById("status");
  const ceilingReading = document.getElementById("ceiling-reading");
  const ceilingSummary = document.getElementById("ceiling-summary");
  const scopeInput = document.getElementById("scope-input");
  const scopeReduction = document.getElementById("scope-reduction");
  const scopeOutput = document.getElementById("scope-output");
  const scopeOutputBox = document.getElementById("scope-output-box");
  const historyLine = document.getElementById("history-line");
  const historyFill = document.getElementById("history-fill");

  const state = {
    surface: "play",
    schema: null,
    values: new Map(),
    controls: new Map(),
    queue: new Map(),
    writtenAt: new Map(),
    held: new Set(),
    sounds: [],
    selectedSoundId: "",
    edited: false,
    built: false,
    meters: [],
  };

  const pending = new Map();
  let nextRequest = 1;
  let writeEpoch = 0;
  let writeTimer = null;
  let writing = false;
  let lastWriteAt = 0;
  let refreshTimer = null;
  let statusTimer = null;
  let scopeFrame = null;
  const reductionHistory = Array(96).fill(0);

  /* --------------------------------------------------------------- bridge */

  function call(method, params) {
    const requestId = REQUEST_PREFIX + nextRequest++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(requestId);
        reject(new Error("RackForge did not answer in time"));
      }, REQUEST_TIMEOUT);
      pending.set(requestId, {
        resolve: (result) => {
          clearTimeout(timer);
          resolve(result);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      parent.postMessage(
        {
          protocol: PROTOCOL,
          kind: "request",
          request_id: requestId,
          method: method,
          params: params || {},
        },
        "*",
      );
    });
  }

  window.addEventListener("message", (event) => {
    const message = event.data;
    if (!message || message.protocol !== PROTOCOL) return;

    if (message.kind === "context") {
      state.surface = message.surface || "play";
      const instance = message.instance || {};
      state.selectedSoundId = instance.selected_sound_id || state.selectedSoundId;
      state.sounds = instance.sounds || state.sounds;
      renderPresetList();
      scheduleRefresh(120);
      return;
    }

    if (message.kind === "parameter_changed") {
      applyValues(
        [{ index: message.parameter_index, value: message.value }],
        writeEpoch,
      );
      publishSurfaceInfo();
      return;
    }

    if (message.kind !== "response") return;
    const waiting = pending.get(message.request_id);
    if (!waiting) return;
    pending.delete(message.request_id);
    if (message.ok) waiting.resolve(message.result);
    else waiting.reject(new Error(message.error || "RackForge refused the request"));
  });

  /* --------------------------------------------------------------- input */

  function capture(element, pointerId) {
    try {
      element.setPointerCapture(pointerId);
    } catch (error) {
      /* the pointer is gone, or synthetic */
    }
  }

  function releaseCapture(element, pointerId) {
    try {
      element.releasePointerCapture(pointerId);
    } catch (error) {
      /* never captured, or already released */
    }
  }

  const gestures = new Set();

  function endGestures() {
    [...gestures].forEach((finish) => finish());
  }

  window.addEventListener("blur", endGestures);
  window.addEventListener("pagehide", endGestures);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) endGestures();
  });

  /* ---------------------------------------------------------------- status */

  function say(text, isError) {
    statusElement.textContent = text;
    statusElement.classList.toggle("error", Boolean(isError));
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
      statusElement.classList.remove("error");
      statusElement.textContent = connectionLine();
    }, STATUS_LINGER);
  }

  function connectionLine() {
    const sound = state.sounds.find((entry) => entry.id === state.selectedSoundId);
    const name = sound ? sound.name : "Custom setting";
    return name + (state.edited ? " · edited" : "") + " · " + state.surface + " surface";
  }

  function idle() {
    if (statusElement.classList.contains("error")) return;
    statusElement.textContent = connectionLine();
  }

  /* ----------------------------------------------------------------- model */

  function parametersOfPage(pageId) {
    return state.schema.parameters
      .filter((parameter) => parameter.page === pageId)
      .sort((left, right) => (left.order || 0) - (right.order || 0));
  }

  function valueOf(parameter) {
    const stored = state.values.get(parameter.index);
    if (Number.isFinite(stored)) return stored;
    const kind = parameter.kind;
    if (kind.type === "boolean") return kind.default ? 1 : 0;
    if (kind.type === "meter") {
      return parameter.id === "meter.input" || parameter.id === "meter.output"
        ? kind.minimum
        : kind.maximum;
    }
    return kind.default;
  }

  function parameterById(id) {
    return state.schema && state.schema.parameters.find((parameter) => parameter.id === id);
  }

  function valueById(id, fallback) {
    const parameter = parameterById(id);
    return parameter ? valueOf(parameter) : fallback;
  }

  /* ------------------------------------------------------------ write path */

  function write(parameter, value) {
    if (state.values.get(parameter.index) === value) return;
    writeEpoch += 1;
    state.values.set(parameter.index, value);
    state.queue.set(parameter.index, value);
    state.writtenAt.set(parameter.index, writeEpoch);
    scheduleScope();
    if (!state.edited) {
      state.edited = true;
      idle();
    }
    scheduleFlush();
  }

  function scheduleFlush() {
    if (writeTimer !== null || writing) return;
    const due = Math.max(0, WRITE_INTERVAL - (Date.now() - lastWriteAt));
    writeTimer = setTimeout(flush, due);
  }

  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

  async function flush() {
    writeTimer = null;
    if (writing || state.queue.size === 0) return;
    writing = true;
    try {
      while (state.queue.size > 0) {
        const idleFor = Date.now() - lastWriteAt;
        if (idleFor < WRITE_INTERVAL) await pause(WRITE_INTERVAL - idleFor);

        const [index, value] = state.queue.entries().next().value;
        state.queue.delete(index);
        lastWriteAt = Date.now();
        try {
          await call("plugin.set_parameter", { parameter_index: index, value: value });
        } catch (error) {
          const parameter = state.schema.parameters.find((candidate) => candidate.index === index);
          say((parameter ? parameter.name : "That control") + ": " + error.message, true);
        }
      }
    } finally {
      writing = false;
      if (state.queue.size > 0) scheduleFlush();
    }
  }

  /* ------------------------------------------------------------- rendering */

  function formatValue(parameter, value) {
    const kind = parameter.kind;
    if (kind.type === "float" || kind.type === "meter") {
      const unit = kind.unit ? " " + kind.unit : "";
      const span = kind.maximum - kind.minimum;
      const digits = span > 100 ? 0 : span > 4 ? 1 : 2;
      return value.toFixed(digits) + unit;
    }
    if (kind.type === "enum") {
      const choice = kind.choices.find((entry) => entry.value === Math.round(value));
      return choice ? choice.name : String(value);
    }
    if (kind.type === "boolean") return value >= 0.5 ? "on" : "off";
    if (kind.type === "integer") {
      return String(Math.round(value)) + (kind.unit ? " " + kind.unit : "");
    }
    return String(value);
  }

  /** Position 0..1 of a value on its knob, honouring a logarithmic taper. */
  function normalise(kind, value) {
    if (kind.taper === "logarithmic" && kind.minimum > 0) {
      return Math.log(value / kind.minimum) / Math.log(kind.maximum / kind.minimum);
    }
    return (value - kind.minimum) / (kind.maximum - kind.minimum);
  }

  function denormalise(kind, position) {
    const clamped = Math.min(1, Math.max(0, position));
    if (kind.taper === "logarithmic" && kind.minimum > 0) {
      return kind.minimum * Math.pow(kind.maximum / kind.minimum, clamped);
    }
    return kind.minimum + clamped * (kind.maximum - kind.minimum);
  }

  function knobControl(parameter) {
    const kind = parameter.kind;
    const wrapper = document.createElement("div");
    wrapper.className = "knob";

    const size = 52;
    const radius = 20;
    const centre = size / 2;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 " + size + " " + size);
    svg.setAttribute("width", String(size));
    svg.setAttribute("height", String(size));
    svg.setAttribute("role", "slider");
    svg.setAttribute("tabindex", "0");
    svg.setAttribute("aria-label", parameter.name);
    svg.setAttribute("aria-valuemin", String(kind.minimum));
    svg.setAttribute("aria-valuemax", String(kind.maximum));

    const track = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    track.setAttribute("cx", String(centre));
    track.setAttribute("cy", String(centre));
    track.setAttribute("r", String(radius));
    track.setAttribute("class", "knob-body");
    svg.appendChild(track);

    const sweep = document.createElementNS("http://www.w3.org/2000/svg", "path");
    sweep.setAttribute("class", "knob-sweep");
    sweep.setAttribute("fill", "none");
    svg.appendChild(sweep);

    const pointer = document.createElementNS("http://www.w3.org/2000/svg", "line");
    pointer.setAttribute("class", "knob-pointer");
    svg.appendChild(pointer);

    const label = document.createElement("div");
    label.className = "label";
    label.textContent = parameter.name;
    const reading = document.createElement("div");
    reading.className = "reading";

    const angleOf = (value) => (-225 + 270 * normalise(kind, value)) * (Math.PI / 180);
    const pointAt = (angle, distance) => [
      centre + Math.cos(angle) * distance,
      centre + Math.sin(angle) * distance,
    ];

    function paint(value) {
      const angle = angleOf(value);
      const [tipX, tipY] = pointAt(angle, radius * 0.78);
      pointer.setAttribute("x1", String(centre));
      pointer.setAttribute("y1", String(centre));
      pointer.setAttribute("x2", String(tipX));
      pointer.setAttribute("y2", String(tipY));

      const start = angleOf(kind.minimum);
      const [startX, startY] = pointAt(start, radius * 0.92);
      const [endX, endY] = pointAt(angle, radius * 0.92);
      const large = 270 * normalise(kind, value) > 180 ? 1 : 0;
      sweep.setAttribute(
        "d",
        "M " + startX + " " + startY +
          " A " + radius * 0.92 + " " + radius * 0.92 + " 0 " + large + " 1 " + endX + " " + endY,
      );

      reading.textContent = formatValue(parameter, value);
      svg.setAttribute("aria-valuenow", String(value));
      svg.setAttribute("aria-valuetext", reading.textContent);
    }

    function quantise(value) {
      const step = kind.step || 0.001;
      const stepped = Math.round(value / step) * step;
      return Math.min(kind.maximum, Math.max(kind.minimum, stepped));
    }

    let dragging = false;
    let pointerId = null;
    let startY = 0;
    let startPosition = 0;

    svg.addEventListener("pointerdown", (event) => {
      dragging = true;
      pointerId = event.pointerId;
      startY = event.clientY;
      startPosition = normalise(kind, valueOf(parameter));
      state.held.add(parameter.index);
      capture(svg, pointerId);
      svg.classList.add("held");
      gestures.add(finish);
      event.preventDefault();
    });

    svg.addEventListener("pointermove", (event) => {
      if (!dragging) return;
      // Two hundred pixels covers the range; shift slows it to a crawl for the
      // settings that need it.
      const scale = event.shiftKey ? 800 : 200;
      const position = startPosition + (startY - event.clientY) / scale;
      const value = quantise(denormalise(kind, position));
      paint(value);
      write(parameter, value);
    });

    function finish() {
      if (!dragging) return;
      dragging = false;
      gestures.delete(finish);
      state.held.delete(parameter.index);
      svg.classList.remove("held");
      releaseCapture(svg, pointerId);
      scheduleRefresh();
    }

    svg.addEventListener("pointerup", finish);
    svg.addEventListener("pointercancel", finish);
    svg.addEventListener("lostpointercapture", finish);

    svg.addEventListener("dblclick", () => {
      paint(kind.default);
      write(parameter, kind.default);
    });

    svg.addEventListener("keydown", (event) => {
      const stepPosition = 1 / (event.shiftKey ? 200 : 20);
      let position = normalise(kind, valueOf(parameter));
      if (event.key === "ArrowUp" || event.key === "ArrowRight") position += stepPosition;
      else if (event.key === "ArrowDown" || event.key === "ArrowLeft") position -= stepPosition;
      else if (event.key === "Home") position = 0;
      else if (event.key === "End") position = 1;
      else return;
      event.preventDefault();
      const value = quantise(denormalise(kind, position));
      paint(value);
      write(parameter, value);
    });

    paint(valueOf(parameter));
    wrapper.append(svg, label, reading);
    state.controls.set(parameter.index, { apply: paint });
    return wrapper;
  }

  function toggleControl(parameter) {
    const wrapper = document.createElement("div");
    wrapper.className = "knob";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "toggle" + (parameter.id === "output.delta_listen" ? " delta" : "");
    button.textContent = parameter.name;
    button.setAttribute("aria-label", parameter.name);
    button.addEventListener("click", () => {
      const next = valueOf(parameter) >= 0.5 ? 0 : 1;
      paint(next);
      write(parameter, next);
    });
    const reading = document.createElement("div");
    reading.className = "reading";

    function paint(value) {
      const on = value >= 0.5;
      button.setAttribute("aria-pressed", String(on));
      reading.textContent = on ? "on" : "off";
    }

    paint(valueOf(parameter));
    wrapper.append(button, reading);
    state.controls.set(parameter.index, { apply: paint });
    return wrapper;
  }

  function choiceControl(parameter) {
    const wrapper = document.createElement("div");
    wrapper.className = "knob wide";
    const label = document.createElement("div");
    label.className = "label";
    label.textContent = parameter.name;
    const row = document.createElement("div");
    row.className = "choice";
    row.setAttribute("role", "radiogroup");
    row.setAttribute("aria-label", parameter.name);

    const buttons = parameter.kind.choices.map((choice) => {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = choice.name;
      button.setAttribute("role", "radio");
      button.addEventListener("click", () => {
        paint(choice.value);
        write(parameter, choice.value);
      });
      row.appendChild(button);
      return button;
    });

    function paint(value) {
      const selected = Math.round(value);
      buttons.forEach((button, index) => {
        const isSelected = parameter.kind.choices[index].value === selected;
        button.setAttribute("aria-checked", String(isSelected));
        button.setAttribute("aria-pressed", String(isSelected));
      });
    }

    paint(valueOf(parameter));
    wrapper.append(label, row);
    state.controls.set(parameter.index, { apply: paint });
    return wrapper;
  }

  /** A reading from the engine: reduction retreats from the right; levels rise from the left. */
  function meterControl(parameter) {
    const kind = parameter.kind;
    const wrapper = document.createElement("div");
    const levelMeter = parameter.id === "meter.input" || parameter.id === "meter.output";
    wrapper.className = "knob wide meter" + (levelMeter ? " level" : " reduction");
    const label = document.createElement("div");
    label.className = "label";
    label.textContent = parameter.name;
    const track = document.createElement("div");
    track.className = "meter-track";
    track.setAttribute("role", "meter");
    track.setAttribute("aria-label", parameter.name);
    track.setAttribute("aria-valuemin", String(kind.minimum));
    track.setAttribute("aria-valuemax", String(kind.maximum));
    const fill = document.createElement("div");
    fill.className = "meter-fill";
    track.appendChild(fill);
    const scale = document.createElement("div");
    scale.className = "meter-scale";
    const low = document.createElement("span");
    low.textContent = formatValue(parameter, kind.minimum);
    const reading = document.createElement("span");
    reading.className = "reading";
    const high = document.createElement("span");
    high.textContent = formatValue(parameter, kind.maximum);
    scale.append(low, reading, high);

    function paint(value) {
      const clamped = Math.min(kind.maximum, Math.max(kind.minimum, value));
      const fraction = levelMeter
        ? (clamped - kind.minimum) / (kind.maximum - kind.minimum)
        : (kind.maximum - clamped) / (kind.maximum - kind.minimum);
      fill.style.width = (fraction * 100).toFixed(1) + "%";
      reading.textContent = formatValue(parameter, clamped);
      track.setAttribute("aria-valuenow", String(clamped));
    }

    paint(valueOf(parameter));
    wrapper.append(label, track, scale);
    state.controls.set(parameter.index, { apply: paint });
    state.meters.push(parameter);
    return wrapper;
  }

  function control(parameter) {
    switch (parameter.kind.type) {
      case "enum":
        return choiceControl(parameter);
      case "boolean":
        return toggleControl(parameter);
      case "meter":
        return meterControl(parameter);
      default:
        return knobControl(parameter);
    }
  }

  function groupCard(page) {
    const card = document.createElement("section");
    card.className = "group";
    card.dataset.page = page.id;
    card.setAttribute("aria-label", page.name);
    const name = document.createElement("div");
    name.className = "group-name";
    name.textContent = page.name;
    const knobs = document.createElement("div");
    knobs.className = "knobs";
    parametersOfPage(page.id).forEach((parameter) => knobs.appendChild(control(parameter)));
    card.append(name, knobs);
    return card;
  }

  /* --------------------------------------------------------- live scope */

  function displayDb(value, unit) {
    const safe = Number.isFinite(value) ? value : -60;
    return safe.toFixed(1).replace("-", "−") + " " + unit;
  }

  function renderScope() {
    scopeFrame = null;
    if (!state.schema) return;
    const ceiling = valueById("limiter.ceiling", -1);
    const input = valueById("meter.input", -60);
    const reduction = valueById("output.reduction", 0);
    const output = valueById("meter.output", -60);
    const truePeak = valueById("limiter.true_peak", 1) >= 0.5;
    const linked = valueById("limiter.link", 1) >= 0.5;
    const delta = valueById("output.delta_listen", 0) >= 0.5;
    const lookahead = valueById("limiter.lookahead", 2);

    ceilingReading.textContent = displayDb(ceiling, "dBTP");
    ceilingSummary.textContent = delta
      ? "Delta listen · removed signal only"
      : (truePeak ? "True peak" : "Sample peak") + " · " +
        (linked ? "linked" : "dual mono") + " · " + lookahead.toFixed(1) + " ms ahead";
    scopeInput.textContent = displayDb(input, "dB");
    scopeReduction.textContent = displayDb(reduction, "dB");
    scopeOutput.textContent = displayDb(output, "dB");
    scopeOutputBox.classList.toggle("safe", output <= ceiling + 0.2);

    const points = reductionHistory.map((value, index) => {
      const x = 5 + (index / (reductionHistory.length - 1)) * 190;
      const amount = Math.min(30, Math.max(0, -value));
      const y = 6 + (amount / 30) * 68;
      return [x, y];
    });
    const path = points.map((point, index) =>
      (index === 0 ? "M " : " L ") + point[0].toFixed(2) + " " + point[1].toFixed(2),
    ).join("");
    historyLine.setAttribute("d", path);
    historyFill.setAttribute("d", path + " L 195 6 L 5 6 Z");
  }

  function scheduleScope() {
    if (scopeFrame !== null) return;
    scopeFrame = requestAnimationFrame(renderScope);
  }

  /* -------------------------------------------------------------- meters */

  let publishedInfo = "";
  function publishSurfaceInfo() {
    if (state.meters.length === 0) return;
    const first = state.meters[0];
    const value = formatValue(first, valueOf(first));
    if (value === publishedInfo) return;
    publishedInfo = value;
    call("plugin.set_surface_info", { label: SURFACE_LABEL, value: value }).catch(() => undefined);
  }

  /* ------------------------------------------------------------- presets */

  function renderPresetList() {
    const current = presetElement.value;
    presetElement.textContent = "";
    const placeholder = document.createElement("option");
    placeholder.value = "";
    placeholder.textContent = state.sounds.length ? "Factory settings…" : "No factory settings";
    presetElement.appendChild(placeholder);
    state.sounds.forEach((sound) => {
      const option = document.createElement("option");
      option.value = sound.id;
      option.textContent = sound.name;
      option.title = sound.detail || "";
      presetElement.appendChild(option);
    });
    presetElement.value = state.selectedSoundId || current || "";
  }

  presetElement.addEventListener("change", () => {
    const soundId = presetElement.value;
    if (!soundId) return;
    call("plugin.select_sound", { sound_id: soundId })
      .then(() => {
        state.selectedSoundId = soundId;
        state.edited = false;
        say("Loaded " + presetElement.selectedOptions[0].textContent, false);
        return refresh();
      })
      .catch((error) => say("Could not load that setting: " + error.message, true));
  });

  /* --------------------------------------------------------------- wiring */

  function applyValues(values, readEpoch) {
    (values || []).forEach((entry) => {
      if (!Number.isInteger(entry.index) || !Number.isFinite(entry.value)) return;
      if (state.held.has(entry.index)) return;
      if ((state.writtenAt.get(entry.index) || 0) > readEpoch) return;
      state.values.set(entry.index, entry.value);
      const parameter = state.schema.parameters.find((candidate) => candidate.index === entry.index);
      if (parameter && parameter.id === "output.reduction") {
        reductionHistory.push(entry.value);
        reductionHistory.shift();
      }
      const widget = state.controls.get(entry.index);
      if (widget) widget.apply(entry.value);
    });
    scheduleScope();
  }

  function scheduleRefresh(delay) {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(refresh, delay === undefined ? 150 : delay);
  }

  async function refresh(quiet) {
    if (state.queue.size > 0 || writing || state.held.size > 0) {
      if (!quiet) scheduleRefresh();
      return;
    }
    const readEpoch = writeEpoch;
    try {
      const parameters = await call("plugin.parameters");
      const schemaChanged =
        !state.schema ||
        state.schema.parameters.length !== parameters.schema.parameters.length;
      state.schema = parameters.schema;
      if (!state.built || schemaChanged) {
        build();
      }
      applyValues(parameters.values, readEpoch);
      publishSurfaceInfo();
      if (!quiet) idle();
    } catch (error) {
      if (!quiet) say("RackForge did not answer: " + error.message, true);
    }
  }

  function build() {
    state.controls.clear();
    state.meters = [];
    state.writtenAt.clear();
    panelElement.textContent = "";
    [...state.schema.pages]
      .sort((left, right) => (left.order || 0) - (right.order || 0))
      .forEach((page) => panelElement.appendChild(groupCard(page)));
    state.built = true;
    scheduleScope();
  }

  parent.postMessage({ protocol: PROTOCOL, kind: "ready" }, "*");
})();
