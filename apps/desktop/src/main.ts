import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

type SubtitleStyle = {
  position: string;
  fontSize: number;
  textColor: string;
  backgroundColor: string;
  roundedRequired: boolean;
  roundedRadius: number;
  boxPadding: number;
  bgOpacity: number;
  xPaddingScale: number;
};

type ProjectConfig = {
  inputVideo: string;
  clippedVideo: string;
  renderedVideo: string;
  audioWav: string;
  toolApiOrigin: string;
  dashscopeApiKey: string;
  dashscopeBaseUrl: string;
  correctionModel: string;
  asrModel: string;
  whisperJson: string;
  subtitlesJson: string;
  correctedJson: string;
  referenceScript: string;
  language: string;
  cutStart: number;
  cutDuration: number;
  subtitleStyle: SubtitleStyle;
};

type SubtitleSegment = {
  start: number;
  end: number;
  text: string;
};

type SubtitleTrack = {
  language: string;
  segments: SubtitleSegment[];
};

type CorrectionRuntimeConfig = {
  baseUrl: string;
  model: string;
  batchSize: number;
  concurrency: number;
  maxTokens: number;
};

type AsrRuntimeConfig = {
  apiOrigin: string;
  dashscopeApiKey: string;
  dashscopeBaseUrl: string;
  correctionModel: string;
};

type ExportProgressPayload = {
  percent: number;
  text: string;
};

type ExportResult = {
  route: string;
  output: string;
};

const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
let currentTrack: SubtitleTrack | null = null;
let currentTrackPath = "";
let activeSegmentIndex = -1;
let overlayEnabled = true;

function extSwap(path: string, suffix: string): string {
  if (!path) return "";
  const base = path.replace(/\.[^/.]+$/, "");
  return `${base}${suffix}`;
}

function log(message: string) {
  const box = el<HTMLPreElement>("log");
  const now = new Date().toLocaleTimeString();
  box.textContent = `[${now}] ${message}\n` + box.textContent;
}

function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "toString" in e) return String(e);
  return "未知错误";
}

async function step(name: string, fn: () => Promise<void>) {
  const t0 = performance.now();
  log(`[开始] ${name}`);
  const ticker = window.setInterval(() => {
    log(`[进行中] ${name}，已耗时 ${((performance.now() - t0) / 1000).toFixed(1)}s`);
  }, 1000);
  try {
    await fn();
    window.clearInterval(ticker);
    log(`[完成] ${name}，耗时 ${((performance.now() - t0) / 1000).toFixed(2)}s`);
  } catch (e) {
    window.clearInterval(ticker);
    log(`[失败] ${name}，耗时 ${((performance.now() - t0) / 1000).toFixed(2)}s，错误: ${errText(e)}`);
    throw e;
  }
}

function setOverlayText(text: string) {
  const overlay = el<HTMLDivElement>("subtitleOverlay");
  const content = overlayEnabled ? (text || "").replace(/\s+/g, " ").trim() : "";
  overlay.textContent = content;
  overlay.style.visibility = content ? "visible" : "hidden";
  if (!content) return;
  requestAnimationFrame(() => fitOverlayByVideoWidth(content));
}

function setOverlayEnabled(enabled: boolean) {
  overlayEnabled = enabled;
  const overlay = el<HTMLDivElement>("subtitleOverlay");
  overlay.style.display = enabled ? "block" : "none";
  overlay.style.visibility = enabled ? "visible" : "hidden";
  if (!enabled) {
    overlay.textContent = "";
  }
}

function showExportModal() {
  const modal = el<HTMLDivElement>("exportModal");
  modal.classList.remove("hidden");
  modal.setAttribute("aria-hidden", "false");
}

function hideExportModal() {
  const modal = el<HTMLDivElement>("exportModal");
  modal.classList.add("hidden");
  modal.setAttribute("aria-hidden", "true");
}

function setExportProgress(percent: number, text?: string) {
  const box = el<HTMLDivElement>("exportProgressBox");
  const bar = el<HTMLDivElement>("exportProgressBar");
  const label = el<HTMLSpanElement>("exportProgressText");
  box.classList.remove("hidden");
  const value = Math.max(0, Math.min(100, Math.round(percent)));
  bar.style.width = `${value}%`;
  label.textContent = text?.trim() || `${value}%`;
}

function resetExportProgress() {
  const box = el<HTMLDivElement>("exportProgressBox");
  const bar = el<HTMLDivElement>("exportProgressBar");
  const label = el<HTMLSpanElement>("exportProgressText");
  box.classList.add("hidden");
  bar.style.width = "0%";
  label.textContent = "0%";
}

function readSubtitleStyle(): SubtitleStyle {
  const xPaddingScale = Number(el<HTMLInputElement>("xPaddingScale").value || 1);
  return {
    position: el<HTMLSelectElement>("position").value,
    fontSize: Number(el<HTMLInputElement>("fontSize").value || 17),
    textColor: el<HTMLInputElement>("textColor").value,
    backgroundColor: el<HTMLInputElement>("bgColor").value,
    roundedRequired: el<HTMLInputElement>("roundedRequired").checked,
    roundedRadius: Number(el<HTMLInputElement>("roundedRadius").value || 16),
    boxPadding: Number(el<HTMLInputElement>("boxPadding").value || 18),
    bgOpacity: Number(el<HTMLInputElement>("bgOpacity").value || 68),
    xPaddingScale: Math.min(1, Math.max(0.5, xPaddingScale)),
  };
}

function applySubtitleStyleToOverlay(style: SubtitleStyle) {
  const overlay = el<HTMLDivElement>("subtitleOverlay");
  overlay.style.fontSize = `${Math.max(12, style.fontSize)}px`;
  overlay.style.color = style.textColor;
  const opacity = Math.min(100, Math.max(0, style.bgOpacity)) / 100;
  const hex = style.backgroundColor.replace("#", "");
  const r = parseInt(hex.slice(0, 2), 16) || 0;
  const g = parseInt(hex.slice(2, 4), 16) || 0;
  const b = parseInt(hex.slice(4, 6), 16) || 0;
  const scale = Math.min(1, Math.max(0.5, style.xPaddingScale));
  overlay.style.backgroundColor = `rgba(${r}, ${g}, ${b}, ${opacity.toFixed(2)})`;
  overlay.style.padding = `${Math.max(4, style.boxPadding)}px ${Math.max(6, Math.round(style.boxPadding * 1.3 * scale))}px`;
  overlay.style.borderRadius = `${Math.max(2, style.roundedRadius)}px`;
  overlay.style.top = "";
  overlay.style.bottom = "";
  overlay.style.transform = "translateX(-50%) scale(var(--overlay-scale, 1))";
  overlay.style.transformOrigin = "center bottom";
  if (style.position === "top") {
    overlay.style.top = "36px";
    overlay.style.transformOrigin = "center top";
  } else if (style.position === "center") {
    overlay.style.top = "50%";
    overlay.style.transform = "translate(-50%, -50%) scale(var(--overlay-scale, 1))";
    overlay.style.transformOrigin = "center center";
  } else {
    overlay.style.bottom = "36px";
  }
  const content = (overlay.textContent || "").trim();
  if (content) {
    requestAnimationFrame(() => fitOverlayByVideoWidth(content));
  }
}

function fitOverlayByVideoWidth(content: string) {
  const overlay = el<HTMLDivElement>("subtitleOverlay");
  const video = el<HTMLVideoElement>("previewVideo");
  const videoWidth = video.clientWidth || 0;
  if (!videoWidth || !content) {
    overlay.style.setProperty("--overlay-scale", "1");
    return;
  }
  const computed = getComputedStyle(overlay);
  const font = computed.font || `${computed.fontWeight} ${computed.fontSize} ${computed.fontFamily}`;
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    overlay.style.setProperty("--overlay-scale", "1");
    return;
  }
  ctx.font = font;
  const textWidth = ctx.measureText(content).width;
  const padLeft = parseFloat(computed.paddingLeft || "0") || 0;
  const padRight = parseFloat(computed.paddingRight || "0") || 0;
  const requiredWidth = textWidth + padLeft + padRight;
  const maxWidth = Math.max(160, videoWidth * 0.9);
  const ratio = maxWidth / Math.max(requiredWidth, 1);
  const scale = Math.min(1, Math.max(0.58, ratio));
  overlay.style.setProperty("--overlay-scale", scale.toFixed(3));
}

function syncStylePanelPreview(style: SubtitleStyle) {
  const textColor = (style.textColor || "#ffe200").toUpperCase();
  const bgColor = (style.backgroundColor || "#000000").toUpperCase();
  el<HTMLSpanElement>("textColorPreview").style.backgroundColor = textColor;
  el<HTMLSpanElement>("textColorValue").textContent = textColor;
  el<HTMLSpanElement>("bgColorPreview").style.backgroundColor = bgColor;
  el<HTMLSpanElement>("bgColorValue").textContent = bgColor;
}

function setActiveSegment(index: number, scroll = false) {
  activeSegmentIndex = index;
  if (index < 0) return;
  const cur = document.querySelector(`.segment-item[data-index="${activeSegmentIndex}"]`) as HTMLElement | null;
  if (cur && scroll) cur.scrollIntoView({ block: "nearest" });
}

function syncSubtitleByPlayerTime(scroll = false) {
  if (!currentTrack || !currentTrack.segments.length) {
    setOverlayText("");
    setActiveSegment(-1);
    return;
  }

  const t = el<HTMLVideoElement>("previewVideo").currentTime;
  const idx = currentTrack.segments.findIndex((seg) => t >= seg.start && t <= seg.end);
  if (idx < 0) {
    setOverlayText("");
    setActiveSegment(-1);
    return;
  }

  setActiveSegment(idx, scroll);
  setOverlayText(currentTrack.segments[idx].text || "");
}

function jumpToSegment(index: number) {
  if (!currentTrack || index < 0 || index >= currentTrack.segments.length) return;
  const seg = currentTrack.segments[index];
  const player = el<HTMLVideoElement>("previewVideo");
  player.currentTime = Math.max(seg.start + 0.01, 0);
  player.pause();
  setActiveSegment(index, true);
  if (overlayEnabled) setOverlayText(seg.text || "");
  log(`已定位到字幕 #${index + 1} (${secFormat(seg.start)}-${secFormat(seg.end)})`);
}

function setupVideoSource(path: string) {
  const player = el<HTMLVideoElement>("previewVideo");
  if (!path) {
    player.removeAttribute("src");
    player.load();
    setOverlayText("");
    return;
  }

  const toFileUrl = (p: string) => {
    const normalized = p.replace(/\\/g, "/");
    const encoded = normalized
      .split("/")
      .map((seg, i) => (i === 0 ? seg : encodeURIComponent(seg)))
      .join("/");
    return `file://${encoded}`;
  };

  const primary = convertFileSrc(path);
  const fallback = toFileUrl(path);
  let switched = false;

  log(`预览源(primary): ${primary}`);

  player.onerror = () => {
    const mediaErr = player.error;
    log(`预览加载失败: code=${mediaErr?.code ?? "unknown"}, src=${player.currentSrc || player.src}`);
    if (!switched) {
      switched = true;
      log("尝试预览回退到 file:// 协议");
      player.src = fallback;
      player.load();
    }
  };

  player.onloadedmetadata = () => {
    log(`预览已加载: ${(player.duration || 0).toFixed(2)}s`);
    syncSubtitleByPlayerTime();
    const current = (el<HTMLDivElement>("subtitleOverlay").textContent || "").trim();
    if (current) fitOverlayByVideoWidth(current);
  };

  player.ontimeupdate = () => syncSubtitleByPlayerTime();
  player.onseeked = () => syncSubtitleByPlayerTime(true);

  player.src = primary;
  player.load();
}

function isDerivedFromInput(value: string, input: string, suffix: string): boolean {
  if (!value || !input) return false;
  return value === extSwap(input, suffix);
}

function fillDerivedPaths(options?: { force?: boolean; previousInput?: string }) {
  const input = el<HTMLInputElement>("inputVideo").value.trim();
  if (!input) return;
  const force = Boolean(options?.force);
  const previousInput = options?.previousInput?.trim() || "";

  const syncField = (id: string, suffix: string) => {
    const node = el<HTMLInputElement>(id);
    const cur = node.value.trim();
    const replaceBecauseOldDerived =
      !!previousInput && isDerivedFromInput(cur, previousInput, suffix);
    if (force || !cur || replaceBecauseOldDerived) {
      node.value = extSwap(input, suffix);
    }
  };
  syncField("audioWav", ".audio.wav");
  syncField("whisperJson", ".asr.raw.json");
  syncField("subtitlesJson", ".asr.json");
  syncField("correctedJson", ".asr.corrected.json");
  syncField("renderedVideo", ".subtitled.mp4");

  // compatibility fields
  el<HTMLInputElement>("clippedVideo").value = extSwap(input, ".clip.mp4");
  el<HTMLInputElement>("cutStart").value = "0";
  el<HTMLInputElement>("cutDuration").value = "0";
}

function getProject(): ProjectConfig {
  return {
    inputVideo: el<HTMLInputElement>("inputVideo").value.trim(),
    clippedVideo: el<HTMLInputElement>("clippedVideo").value.trim(),
    renderedVideo: el<HTMLInputElement>("renderedVideo").value.trim(),
    audioWav: el<HTMLInputElement>("audioWav").value.trim(),
    toolApiOrigin: "",
    dashscopeApiKey: el<HTMLInputElement>("dashscopeApiKey").value.trim(),
    dashscopeBaseUrl: el<HTMLInputElement>("dashscopeBaseUrl").value.trim(),
    correctionModel: el<HTMLInputElement>("correctionModel").value.trim() || "qwen-plus-2025-07-28",
    asrModel: el<HTMLInputElement>("asrModel").value.trim() || "fun-asr",
    whisperJson: el<HTMLInputElement>("whisperJson").value.trim(),
    subtitlesJson: el<HTMLInputElement>("subtitlesJson").value.trim(),
    correctedJson: el<HTMLInputElement>("correctedJson").value.trim(),
    referenceScript: el<HTMLInputElement>("referenceScript").value.trim(),
    language: el<HTMLInputElement>("language").value.trim() || "zh",
    cutStart: 0,
    cutDuration: 0,
    subtitleStyle: readSubtitleStyle(),
  };
}

function secFormat(s: number): string {
  const m = Math.floor(s / 60);
  const ss = (s % 60).toFixed(2).padStart(5, "0");
  return `${String(m).padStart(2, "0")}:${ss}`;
}

function parseSecInput(value: string, fallback: number): number {
  const v = value.trim();
  if (!v) return fallback;
  if (v.includes(":")) {
    const parts = v.split(":").map((p) => p.trim());
    if (parts.length === 2) {
      const m = Number(parts[0]);
      const s = Number(parts[1]);
      if (Number.isFinite(m) && Number.isFinite(s)) return Math.max(0, m * 60 + s);
    }
    if (parts.length === 3) {
      const h = Number(parts[0]);
      const m = Number(parts[1]);
      const s = Number(parts[2]);
      if (Number.isFinite(h) && Number.isFinite(m) && Number.isFinite(s)) return Math.max(0, h * 3600 + m * 60 + s);
    }
  }
  const sec = Number(v);
  return Number.isFinite(sec) ? Math.max(0, sec) : fallback;
}

function renderSegments(track: SubtitleTrack) {
  track.segments = (track.segments || []).map((seg) => ({
    ...seg,
    text: (seg.text || "").replace(/\s+/g, " ").trim(),
  }));
  currentTrack = track;
  currentTrackPath = currentTrackPath || "";
  const list = el<HTMLDivElement>("subtitleList");
  if (!track.segments?.length) {
    list.innerHTML = `<div class="segment-empty">暂无字幕</div>`;
    setOverlayText("");
    return;
  }

  const escapeAttr = (value: string) =>
    (value ?? "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");

  list.innerHTML = track.segments
    .map(
      (seg, i) => `
      <article class="segment-item" data-index="${i}">
        <div class="segment-time-line">
          <span class="segment-index">#${i + 1}</span>
          <span class="segment-time-editor">
            <input class="segment-time-input segment-time-start" data-index="${i}" value="${secFormat(seg.start)}" />
            <span class="segment-time-sep">~</span>
            <input class="segment-time-input segment-time-end" data-index="${i}" value="${secFormat(seg.end)}" />
          </span>
        </div>
        <input
          class="segment-text-input"
          data-index="${i}"
          value="${escapeAttr((seg.text || "").replace(/\s+/g, " ").trim())}"
          spellcheck="false"
        />
      </article>
    `,
    )
    .join("");

  list.querySelectorAll(".segment-item").forEach((node) => {
    node.addEventListener("click", (e) => {
      const item = node as HTMLElement;
      const idx = Number(item.dataset.index || -1);
      jumpToSegment(idx);
      if (e.target === item) {
        const input = item.querySelector(".segment-text-input") as HTMLInputElement | null;
        input?.focus();
        input?.select();
      }
    });
  });

  list.querySelectorAll(".segment-text-input").forEach((node) => {
    node.addEventListener("click", (e) => {
      e.stopPropagation();
      const idx = Number((e.target as HTMLInputElement).dataset.index || -1);
      jumpToSegment(idx);
    });
    node.addEventListener("focus", (e) => {
      const idx = Number((e.target as HTMLInputElement).dataset.index || -1);
      setActiveSegment(idx, true);
    });
    node.addEventListener("input", (e) => {
      const input = e.target as HTMLInputElement;
      const idx = Number(input.dataset.index || -1);
      if (!currentTrack || idx < 0 || idx >= currentTrack.segments.length) return;
      const value = input.value.replace(/\s+/g, " ").trim();
      currentTrack.segments[idx].text = value;
      if (idx === activeSegmentIndex) setOverlayText(value);
    });
  });

  const onTimeChanged = (target: HTMLInputElement, isStart: boolean) => {
    const idx = Number(target.dataset.index || -1);
    if (!currentTrack || idx < 0 || idx >= currentTrack.segments.length) return;
    const seg = currentTrack.segments[idx];
    const nextValue = parseSecInput(target.value, isStart ? seg.start : seg.end);
    if (isStart) {
      seg.start = Math.min(nextValue, Math.max(0, seg.end - 0.01));
    } else {
      seg.end = Math.max(nextValue, seg.start + 0.01);
    }
    const startNode = document.querySelector(`.segment-time-start[data-index="${idx}"]`) as HTMLInputElement | null;
    const endNode = document.querySelector(`.segment-time-end[data-index="${idx}"]`) as HTMLInputElement | null;
    if (startNode) startNode.value = secFormat(seg.start);
    if (endNode) endNode.value = secFormat(seg.end);
    syncSubtitleByPlayerTime();
  };

  const adjustTimeByStep = (target: HTMLInputElement, isStart: boolean, delta: number) => {
    const idx = Number(target.dataset.index || -1);
    if (!currentTrack || idx < 0 || idx >= currentTrack.segments.length) return;
    const seg = currentTrack.segments[idx];
    const baseValue = isStart ? seg.start : seg.end;
    target.value = secFormat(Math.max(0, baseValue + delta));
    onTimeChanged(target, isStart);
    target.select();
  };

  const focusSiblingTimeInput = (target: HTMLInputElement, forward: boolean) => {
    const idx = Number(target.dataset.index || -1);
    const selector = forward ? ".segment-time-end" : ".segment-time-start";
    let nextNode: HTMLInputElement | null = null;
    if (target.classList.contains("segment-time-start") && forward) {
      nextNode = document.querySelector(`${selector}[data-index="${idx}"]`) as HTMLInputElement | null;
    } else if (target.classList.contains("segment-time-end") && !forward) {
      nextNode = document.querySelector(`${selector}[data-index="${idx}"]`) as HTMLInputElement | null;
    }
    if (nextNode) {
      nextNode.focus();
      nextNode.select();
    }
  };

  list.querySelectorAll(".segment-time-input").forEach((node) => {
    node.addEventListener("click", (e) => {
      e.stopPropagation();
      const idx = Number((e.target as HTMLInputElement).dataset.index || -1);
      jumpToSegment(idx);
    });
    node.addEventListener("focus", (e) => {
      e.stopPropagation();
      const target = e.target as HTMLInputElement;
      const idx = Number(target.dataset.index || -1);
      setActiveSegment(idx, true);
      target.select();
    });
    node.addEventListener("change", (e) => {
      const target = e.target as HTMLInputElement;
      onTimeChanged(target, target.classList.contains("segment-time-start"));
    });
    node.addEventListener("keydown", (e) => {
      const event = e as KeyboardEvent;
      const target = e.target as HTMLInputElement;
      const isStart = target.classList.contains("segment-time-start");
      if (event.key === "Enter") {
        onTimeChanged(target, isStart);
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        adjustTimeByStep(target, isStart, 0.1);
      } else if (event.key === "ArrowDown") {
        event.preventDefault();
        adjustTimeByStep(target, isStart, -0.1);
      } else if (event.key === "Tab") {
        if ((isStart && !event.shiftKey) || (!isStart && event.shiftKey)) {
          event.preventDefault();
          focusSiblingTimeInput(target, !event.shiftKey);
        }
      }
    });
  });
}

function activateTab(tabName: string) {
  document.querySelectorAll(".tab-btn").forEach((node) => {
    const btn = node as HTMLButtonElement;
    btn.classList.toggle("active", btn.dataset.tab === tabName);
  });
  document.querySelectorAll(".tab-panel").forEach((node) => {
    const panel = node as HTMLDivElement;
    panel.classList.toggle("active", panel.dataset.tab === tabName);
  });
}

async function loadTrack(path: string) {
  if (!path) {
    renderSegments({ language: "zh", segments: [] });
    return;
  }
  const track = await invoke<SubtitleTrack>("load_subtitles", { path });
  currentTrackPath = path;
  setOverlayEnabled(true);
  renderSegments(track);
  syncSubtitleByPlayerTime(true);
}

async function generateSubtitles() {
  const p = getProject();
  if (!p.inputVideo) throw new Error("请先导入视频");
  if (!p.dashscopeApiKey) throw new Error("请先填写 DASHSCOPE_API_KEY");

  await step("抽取音频", async () => {
    log(`参数: input=${p.inputVideo}`);
    log(`参数: audioOut=${p.audioWav}`);
    await invoke("extract_audio", { input: p.inputVideo, output: p.audioWav });
  });

  await step("云端转写", async () => {
    log(`参数: asrModel=${p.asrModel}`);
    log(`参数: whisperJson=${p.whisperJson}`);
    log(`参数: subtitlesOut=${p.subtitlesJson}`);
    const asrInfo = await invoke<string>("transcribe_audio", {
      dashscopeApiKey: p.dashscopeApiKey,
      asrModel: p.asrModel,
      wav: p.audioWav,
      whisperJson: p.whisperJson,
      subtitlesOut: p.subtitlesJson,
    });
    log(`ASR链路: ${asrInfo}`);
  });

  let targetSubtitlePath = p.subtitlesJson;
  await step("大模型纠正字幕", async () => {
    log(`参数: subtitles=${p.subtitlesJson}`);
    log(`参数: correctedOut=${p.correctedJson}`);
    log(`参数: reference=${p.referenceScript || "<none>"}`);
    log(`参数: dashscopeBaseUrl=${p.dashscopeBaseUrl || "<default>"}`);
    log(`参数: correctionModel=${p.correctionModel}`);
    const runtime = await invoke<CorrectionRuntimeConfig>("get_correction_runtime_config");
    log(
      `纠正配置: baseUrl=${runtime.baseUrl}, model=${runtime.model}, batch=${runtime.batchSize}, concurrency=${runtime.concurrency}, maxTokens=${runtime.maxTokens}`,
    );

    await invoke("correct_subtitles", {
      subtitles: p.subtitlesJson,
      output: p.correctedJson,
      reference: p.referenceScript || null,
      dashscopeApiKey: p.dashscopeApiKey || null,
      dashscopeBaseUrl: p.dashscopeBaseUrl || null,
      correctionModel: p.correctionModel || null,
    });
    targetSubtitlePath = p.correctedJson;
    log("字幕纠正完成（Rust 并发分批）");
  });

  await loadTrack(targetSubtitlePath);
  activateTab("subtitles");
}

function bind() {
  document.querySelectorAll(".tab-btn").forEach((node) => {
    node.addEventListener("click", () => activateTab((node as HTMLButtonElement).dataset.tab || "subtitles"));
  });

  el<HTMLButtonElement>("pickVideo").onclick = async () => {
    const selected = await invoke<string | null>("pick_video_file");
    if (selected) {
      const previousInput = el<HTMLInputElement>("inputVideo").value.trim();
      el<HTMLInputElement>("inputVideo").value = selected;
      fillDerivedPaths({ force: true, previousInput });
      el<HTMLInputElement>("inputVideo").dataset.prevInput = selected;
      setupVideoSource(selected);
      setOverlayEnabled(true);
      log(`输出默认路径: ${el<HTMLInputElement>("renderedVideo").value.trim()}`);
      log(`已导入视频: ${selected}`);
    }
  };

  el<HTMLButtonElement>("pickReference").onclick = async () => {
    const selected = await invoke<string | null>("pick_reference_file");
    if (selected) {
      el<HTMLInputElement>("referenceScript").value = selected;
      log(`已导入参考文本: ${selected}`);
    }
  };

  el<HTMLButtonElement>("btnGenerateSubtitles").onclick = async () => {
    await step("生成字幕流程", async () => {
      await generateSubtitles();
    });
  };

  el<HTMLButtonElement>("btnExport").onclick = async () => {
    const p = getProject();
    const fallback = p.renderedVideo || extSwap(p.inputVideo, ".subtitled.mp4");
    el<HTMLInputElement>("exportPath").value = fallback;
    showExportModal();
  };

  el<HTMLButtonElement>("pickExportPath").onclick = async () => {
    const current = el<HTMLInputElement>("exportPath").value.trim();
    const selected = await invoke<string | null>("pick_export_file", {
      suggestedPath: current || null,
    });
    if (selected) {
      el<HTMLInputElement>("exportPath").value = selected;
    }
  };

  el<HTMLButtonElement>("closeExportModal").onclick = () => {
    resetExportProgress();
    hideExportModal();
  };

  el<HTMLButtonElement>("confirmExport").onclick = async () => {
    setExportProgress(0, "准备导出");
    await step("导出视频", async () => {
      const p = getProject();
      const subtitleForBurn = currentTrackPath || p.correctedJson || p.subtitlesJson;
      const outputVideo = el<HTMLInputElement>("exportPath").value.trim() || p.renderedVideo || extSwap(p.inputVideo, ".subtitled.mp4");
      const resolution = el<HTMLSelectElement>("exportResolution").value;
      const bitrate = el<HTMLInputElement>("exportBitrate").value.trim() || "6M";
      if (!p.inputVideo || !subtitleForBurn) throw new Error("请先准备视频和字幕文件");
      log(`参数: inputVideo=${p.inputVideo}`);
      log(`参数: subtitles=${subtitleForBurn}`);
      log(`参数: outputVideo=${outputVideo}`);
      log(`参数: resolution=${resolution}, bitrate=${bitrate}`);
      log(`参数: roundedRequired=${p.subtitleStyle.roundedRequired}`);
      log(
        `参数: roundedRadius=${p.subtitleStyle.roundedRadius}, boxPadding=${p.subtitleStyle.boxPadding}, bgOpacity=${p.subtitleStyle.bgOpacity}, xPaddingScale=${p.subtitleStyle.xPaddingScale}`,
      );
      const result = await invoke<ExportResult>("export_subtitled_video", {
        inputVideo: p.inputVideo,
        subtitlesJson: subtitleForBurn,
        outputVideo,
        resolution,
        bitrate,
        style: p.subtitleStyle,
      });
      log(`烧录路由: ${result.route}`);
      setupVideoSource(outputVideo);
      setOverlayEnabled(false);
      el<HTMLInputElement>("renderedVideo").value = outputVideo;
      setExportProgress(100, "导出完成 100%");
      hideExportModal();
      log(`导出完成: ${outputVideo}`);
    });
  };

  el<HTMLButtonElement>("btnStripAudio").onclick = async () => {
    await step("剥离音频", async () => {
      const p = getProject();
      if (!p.inputVideo) throw new Error("请先导入视频");
      log(`参数: input=${p.inputVideo}`);
      const outputPath = await invoke<string>("strip_audio", { input: p.inputVideo });
      log(`音频已剥离到: ${outputPath}`);
      // 自动填充到 audioWav 字段
      el<HTMLInputElement>("audioWav").value = outputPath;
    });
  };

  el<HTMLButtonElement>("loadExternalSubtitles").onclick = async () => {
    const selected = await invoke<string | null>("pick_subtitle_file");
    if (!selected) return;
    await step("加载外部字幕", async () => {
      await loadTrack(selected);
      el<HTMLInputElement>("correctedJson").value = selected;
      log(`已加载外部字幕: ${selected}`);
      activateTab("subtitles");
    });
  };

  el<HTMLButtonElement>("loadOriginalSubtitles").onclick = async () => {
    const p = getProject();
    await step("加载识别字幕", async () => {
      await loadTrack(currentTrackPath || p.correctedJson || p.subtitlesJson);
    });
  };

  el<HTMLButtonElement>("saveEditedSubtitles").onclick = async () => {
    if (!currentTrack) {
      log("当前没有可保存的字幕");
      return;
    }
    const p = getProject();
    const targetPath = currentTrackPath || p.correctedJson || p.subtitlesJson;
    if (!targetPath) {
      log("没有可保存路径");
      return;
    }
    await step("保存字幕", async () => {
      await invoke("save_subtitles", { path: targetPath, track: currentTrack });
      log(`字幕已保存: ${targetPath}`);
    });
  };

  el<HTMLInputElement>("inputVideo").addEventListener("change", () => {
    const previousInput = (el<HTMLInputElement>("inputVideo").dataset.prevInput || "").trim();
    fillDerivedPaths({ previousInput });
    setupVideoSource(el<HTMLInputElement>("inputVideo").value.trim());
    setOverlayEnabled(true);
    el<HTMLInputElement>("inputVideo").dataset.prevInput = el<HTMLInputElement>("inputVideo").value.trim();
  });

  const onStyleChanged = () => {
    const style = readSubtitleStyle();
    applySubtitleStyleToOverlay(style);
    syncStylePanelPreview(style);
  };
  ["position", "fontSize", "textColor", "bgColor", "roundedRequired", "roundedRadius", "boxPadding", "bgOpacity", "xPaddingScale"].forEach((id) => {
    const node = el<HTMLElement>(id);
    node.addEventListener("input", onStyleChanged);
    node.addEventListener("change", onStyleChanged);
  });
  el<HTMLVideoElement>("previewVideo").addEventListener("click", () => activateTab("subtitles"));

}

window.addEventListener("DOMContentLoaded", () => {
  bind();
  activateTab("subtitles");
  renderSegments({ language: "zh", segments: [] });
  const style = readSubtitleStyle();
  applySubtitleStyleToOverlay(style);
  syncStylePanelPreview(style);
  log("应用已启动");
  el<HTMLInputElement>("inputVideo").dataset.prevInput = el<HTMLInputElement>("inputVideo").value.trim();
  listen("menu-export", () => {
    const p = getProject();
    el<HTMLInputElement>("exportPath").value = p.renderedVideo || extSwap(p.inputVideo, ".subtitled.mp4");
    resetExportProgress();
    showExportModal();
  }).catch(() => {});
  listen<ExportProgressPayload>("export-progress", (event) => {
    setExportProgress(event.payload.percent, event.payload.text || `${event.payload.percent}%`);
  }).catch((e) => log(`监听导出进度失败: ${errText(e)}`));
  invoke<AsrRuntimeConfig>("get_asr_runtime_config")
    .then((cfg) => {
      const keyInput = el<HTMLInputElement>("dashscopeApiKey");
      if (!keyInput.value.trim() && cfg.dashscopeApiKey) {
        keyInput.value = cfg.dashscopeApiKey;
        log(`已从环境变量读取 DASHSCOPE_API_KEY（长度: ${cfg.dashscopeApiKey.length}）`);
      } else if (!cfg.dashscopeApiKey) {
        log("未从环境变量读取到 DASHSCOPE_API_KEY，可在页面粘贴");
      }
      const baseUrlInput = el<HTMLInputElement>("dashscopeBaseUrl");
      if (!baseUrlInput.value.trim() && cfg.dashscopeBaseUrl) {
        baseUrlInput.value = cfg.dashscopeBaseUrl;
        log(`已读取 DASHSCOPE_BASE_URL: ${cfg.dashscopeBaseUrl}`);
      }
      const correctionModelInput = el<HTMLInputElement>("correctionModel");
      if (!correctionModelInput.value.trim() && cfg.correctionModel) {
        correctionModelInput.value = cfg.correctionModel;
      }
      log(`ASR服务已固定使用: ${cfg.apiOrigin}`);
    })
    .catch((e) => log(`读取 ASR 运行配置失败: ${errText(e)}`));

  window.addEventListener("resize", () => {
    const current = (el<HTMLDivElement>("subtitleOverlay").textContent || "").trim();
    if (current) fitOverlayByVideoWidth(current);
  });
});
