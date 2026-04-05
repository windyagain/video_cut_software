import { convertFileSrc, invoke } from "@tauri-apps/api/core";
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
};

type ProjectConfig = {
  inputVideo: string;
  clippedVideo: string;
  renderedVideo: string;
  audioWav: string;
  whisperBin: string;
  whisperModel: string;
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

type LocalSetup = {
  ffmpegBin?: string | null;
  whisperBin?: string | null;
  whisperModel?: string | null;
  message: string;
};

type CorrectionRuntimeConfig = {
  model: string;
  batchSize: number;
  concurrency: number;
  maxTokens: number;
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
  el<HTMLDivElement>("subtitleOverlay").textContent = overlayEnabled ? text || "" : "";
}

function setOverlayEnabled(enabled: boolean) {
  overlayEnabled = enabled;
  const overlay = el<HTMLDivElement>("subtitleOverlay");
  overlay.style.display = enabled ? "block" : "none";
  if (!enabled) {
    overlay.textContent = "";
  }
}

function readSubtitleStyle(): SubtitleStyle {
  return {
    position: el<HTMLSelectElement>("position").value,
    fontSize: Number(el<HTMLInputElement>("fontSize").value || 17),
    textColor: el<HTMLInputElement>("textColor").value,
    backgroundColor: el<HTMLInputElement>("bgColor").value,
    roundedRequired: el<HTMLInputElement>("roundedRequired").checked,
    roundedRadius: Number(el<HTMLInputElement>("roundedRadius").value || 16),
    boxPadding: Number(el<HTMLInputElement>("boxPadding").value || 18),
    bgOpacity: Number(el<HTMLInputElement>("bgOpacity").value || 68),
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
  overlay.style.backgroundColor = `rgba(${r}, ${g}, ${b}, ${opacity.toFixed(2)})`;
  overlay.style.padding = `${Math.max(4, style.boxPadding)}px ${Math.max(8, Math.round(style.boxPadding * 1.3))}px`;
  overlay.style.borderRadius = `${Math.max(2, style.roundedRadius)}px`;
  overlay.style.top = "";
  overlay.style.bottom = "";
  overlay.style.transform = "translateX(-50%)";
  if (style.position === "top") {
    overlay.style.top = "18px";
  } else if (style.position === "center") {
    overlay.style.top = "50%";
    overlay.style.transform = "translate(-50%, -50%)";
  } else {
    overlay.style.bottom = "18px";
  }
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
  if (activeSegmentIndex === index) return;
  const prev = document.querySelector(`.segment-item[data-index="${activeSegmentIndex}"]`) as HTMLElement | null;
  if (prev) prev.classList.remove("active");
  activeSegmentIndex = index;
  const cur = document.querySelector(`.segment-item[data-index="${activeSegmentIndex}"]`) as HTMLElement | null;
  if (cur) {
    cur.classList.add("active");
    if (scroll) cur.scrollIntoView({ block: "nearest" });
  }
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
  };

  player.ontimeupdate = () => syncSubtitleByPlayerTime();
  player.onseeked = () => syncSubtitleByPlayerTime(true);

  player.src = primary;
  player.load();
}

function fillDerivedPaths() {
  const input = el<HTMLInputElement>("inputVideo").value.trim();
  if (!input) return;

  if (!el<HTMLInputElement>("audioWav").value.trim()) el<HTMLInputElement>("audioWav").value = extSwap(input, ".audio.wav");
  if (!el<HTMLInputElement>("whisperJson").value.trim()) el<HTMLInputElement>("whisperJson").value = extSwap(input, ".asr.raw.json");
  if (!el<HTMLInputElement>("subtitlesJson").value.trim()) el<HTMLInputElement>("subtitlesJson").value = extSwap(input, ".asr.json");
  if (!el<HTMLInputElement>("correctedJson").value.trim()) el<HTMLInputElement>("correctedJson").value = extSwap(input, ".asr.corrected.json");
  if (!el<HTMLInputElement>("renderedVideo").value.trim()) el<HTMLInputElement>("renderedVideo").value = extSwap(input, ".subtitled.mp4");

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
    whisperBin: el<HTMLInputElement>("whisperBin").value.trim(),
    whisperModel: el<HTMLInputElement>("whisperModel").value.trim(),
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

function setProject(project: ProjectConfig) {
  el<HTMLInputElement>("inputVideo").value = project.inputVideo || "";
  el<HTMLInputElement>("renderedVideo").value = project.renderedVideo || "";
  el<HTMLInputElement>("audioWav").value = project.audioWav || "";
  el<HTMLInputElement>("whisperBin").value = project.whisperBin || "";
  el<HTMLInputElement>("whisperModel").value = project.whisperModel || "";
  el<HTMLInputElement>("whisperJson").value = project.whisperJson || "";
  el<HTMLInputElement>("subtitlesJson").value = project.subtitlesJson || "";
  el<HTMLInputElement>("correctedJson").value = project.correctedJson || "";
  el<HTMLInputElement>("referenceScript").value = project.referenceScript || "";
  el<HTMLInputElement>("language").value = project.language || "zh";
  el<HTMLSelectElement>("position").value = project.subtitleStyle?.position || "bottom";
  el<HTMLInputElement>("fontSize").value = String(project.subtitleStyle?.fontSize || 17);
  el<HTMLInputElement>("textColor").value = project.subtitleStyle?.textColor || "#ffe200";
  el<HTMLInputElement>("bgColor").value = project.subtitleStyle?.backgroundColor || "#000000";
  el<HTMLInputElement>("roundedRequired").checked = Boolean(project.subtitleStyle?.roundedRequired);
  el<HTMLInputElement>("roundedRadius").value = String(project.subtitleStyle?.roundedRadius ?? 16);
  el<HTMLInputElement>("boxPadding").value = String(project.subtitleStyle?.boxPadding ?? 18);
  el<HTMLInputElement>("bgOpacity").value = String(project.subtitleStyle?.bgOpacity ?? 68);
  fillDerivedPaths();
  setupVideoSource(project.inputVideo);
  setOverlayEnabled(true);
  const style = readSubtitleStyle();
  applySubtitleStyleToOverlay(style);
  syncStylePanelPreview(style);
}

function applyLocalSetup(setup: LocalSetup) {
  if (setup.whisperBin && !el<HTMLInputElement>("whisperBin").value.trim()) {
    el<HTMLInputElement>("whisperBin").value = setup.whisperBin;
  }
  if (setup.whisperModel && !el<HTMLInputElement>("whisperModel").value.trim()) {
    el<HTMLInputElement>("whisperModel").value = setup.whisperModel;
  }
  el<HTMLDivElement>("setupStatus").textContent = setup.message;
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
  currentTrack = track;
  currentTrackPath = currentTrackPath || "";
  const list = el<HTMLDivElement>("subtitleList");
  if (!track.segments?.length) {
    list.innerHTML = `<div class="segment-empty">暂无字幕</div>`;
    setOverlayText("");
    return;
  }

  list.innerHTML = track.segments
    .map(
      (seg, i) => `
      <article class="segment-item" data-index="${i}">
        <div class="segment-head">
          <span>#${i + 1}</span>
          <span class="segment-time-editor">
            <input class="segment-time-input segment-time-start" data-index="${i}" value="${secFormat(seg.start)}" />
            <span>~</span>
            <input class="segment-time-input segment-time-end" data-index="${i}" value="${secFormat(seg.end)}" />
          </span>
        </div>
        <textarea class="segment-text" data-index="${i}">${seg.text || ""}</textarea>
      </article>
    `,
    )
    .join("");

  list.querySelectorAll(".segment-item").forEach((node) => {
    node.addEventListener("click", () => {
      const idx = Number((node as HTMLElement).dataset.index || -1);
      jumpToSegment(idx);
    });
  });

  list.querySelectorAll(".segment-text").forEach((node) => {
    node.addEventListener("focus", (e) => {
      const idx = Number((e.target as HTMLTextAreaElement).dataset.index || -1);
      jumpToSegment(idx);
    });
    node.addEventListener("input", (e) => {
      const t = e.target as HTMLTextAreaElement;
      const idx = Number(t.dataset.index || -1);
      if (!currentTrack || idx < 0 || idx >= currentTrack.segments.length) return;
      currentTrack.segments[idx].text = t.value;
      if (idx === activeSegmentIndex) setOverlayText(t.value);
    });
  });

  const onTimeChanged = (target: HTMLInputElement, isStart: boolean) => {
    const idx = Number(target.dataset.index || -1);
    if (!currentTrack || idx < 0 || idx >= currentTrack.segments.length) return;
    const seg = currentTrack.segments[idx];
    const raw = parseSecInput(target.value, isStart ? seg.start : seg.end);
    if (isStart) {
      seg.start = Math.min(raw, Math.max(0, seg.end - 0.01));
    } else {
      seg.end = Math.max(raw, seg.start + 0.01);
    }
    target.value = secFormat(isStart ? seg.start : seg.end);
    const startNode = document.querySelector(`.segment-time-start[data-index="${idx}"]`) as HTMLInputElement | null;
    const endNode = document.querySelector(`.segment-time-end[data-index="${idx}"]`) as HTMLInputElement | null;
    if (startNode) startNode.value = secFormat(seg.start);
    if (endNode) endNode.value = secFormat(seg.end);
    syncSubtitleByPlayerTime();
  };

  list.querySelectorAll(".segment-time-start").forEach((node) => {
    node.addEventListener("change", (e) => onTimeChanged(e.target as HTMLInputElement, true));
    node.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key === "Enter") {
        onTimeChanged(e.target as HTMLInputElement, true);
      }
    });
  });

  list.querySelectorAll(".segment-time-end").forEach((node) => {
    node.addEventListener("change", (e) => onTimeChanged(e.target as HTMLInputElement, false));
    node.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key === "Enter") {
        onTimeChanged(e.target as HTMLInputElement, false);
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
  if (!p.whisperBin || !p.whisperModel) throw new Error("未检测到本地转写引擎，请先检测或一键安装");

  await step("抽取音频", async () => {
    log(`参数: input=${p.inputVideo}`);
    log(`参数: audioOut=${p.audioWav}`);
    await invoke("extract_audio", { input: p.inputVideo, output: p.audioWav });
  });

  await step("本地转写", async () => {
    log(`参数: whisperBin=${p.whisperBin}`);
    log(`参数: model=${p.whisperModel}`);
    log(`参数: whisperJson=${p.whisperJson}`);
    log(`参数: subtitlesOut=${p.subtitlesJson}`);
    await invoke("transcribe_audio", {
      whisperBin: p.whisperBin,
      model: p.whisperModel,
      wav: p.audioWav,
      whisperJson: p.whisperJson,
      language: p.language,
      subtitlesOut: p.subtitlesJson,
    });
  });

  let targetSubtitlePath = p.subtitlesJson;
  await step("大模型纠正字幕", async () => {
    log(`参数: subtitles=${p.subtitlesJson}`);
    log(`参数: correctedOut=${p.correctedJson}`);
    log(`参数: reference=${p.referenceScript || "<none>"}`);
    const runtime = await invoke<CorrectionRuntimeConfig>("get_correction_runtime_config");
    log(
      `纠正配置: model=${runtime.model}, batch=${runtime.batchSize}, concurrency=${runtime.concurrency}, maxTokens=${runtime.maxTokens}`,
    );

    await invoke("correct_subtitles", {
      subtitles: p.subtitlesJson,
      output: p.correctedJson,
      reference: p.referenceScript || null,
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
      el<HTMLInputElement>("inputVideo").value = selected;
      fillDerivedPaths();
      setupVideoSource(selected);
      setOverlayEnabled(true);
      const suggested = await invoke<string>("suggest_project_path", { inputVideo: selected });
      el<HTMLInputElement>("projectPath").value = suggested;
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

  el<HTMLButtonElement>("btnBurn").onclick = async () => {
    await step("烧录字幕", async () => {
      const p = getProject();
      const subtitleForBurn = p.correctedJson || p.subtitlesJson;
      const outputVideo = p.renderedVideo || extSwap(p.inputVideo, ".subtitled.mp4");
      if (!p.inputVideo || !subtitleForBurn) throw new Error("请先准备视频和字幕文件");
      log(`参数: inputVideo=${p.inputVideo}`);
      log(`参数: subtitles=${subtitleForBurn}`);
      log(`参数: outputVideo=${outputVideo}`);
      log(`参数: roundedRequired=${p.subtitleStyle.roundedRequired}`);
      log(
        `参数: roundedRadius=${p.subtitleStyle.roundedRadius}, boxPadding=${p.subtitleStyle.boxPadding}, bgOpacity=${p.subtitleStyle.bgOpacity}`,
      );
      await invoke("burn_subtitles", {
        inputVideo: p.inputVideo,
        subtitlesJson: subtitleForBurn,
        outputVideo,
        style: p.subtitleStyle,
      });
      setupVideoSource(outputVideo);
      setOverlayEnabled(false);
      log(`烧录字幕完成: ${outputVideo}`);
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
      await loadTrack(p.subtitlesJson);
    });
  };

  el<HTMLButtonElement>("loadCorrectedSubtitles").onclick = async () => {
    const p = getProject();
    await step("加载纠正字幕", async () => {
      await loadTrack(p.correctedJson);
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

  el<HTMLButtonElement>("btnSave").onclick = async () => {
    let projectPath = el<HTMLInputElement>("projectPath").value.trim();
    if (!projectPath) {
      projectPath = await invoke<string>("suggest_project_path", { inputVideo: getProject().inputVideo || null });
      el<HTMLInputElement>("projectPath").value = projectPath;
    }
    await step("保存项目", async () => {
      await invoke("save_project", { path: projectPath, project: getProject() });
      log(`项目已保存: ${projectPath}`);
    });
  };

  el<HTMLButtonElement>("btnLoad").onclick = async () => {
    const projectPath = el<HTMLInputElement>("projectPath").value.trim();
    await step("加载项目", async () => {
      const project = await invoke<ProjectConfig>("load_project", { path: projectPath });
      setProject(project);
      const targetPath = project.correctedJson || project.subtitlesJson;
      if (targetPath) {
        try {
          await loadTrack(targetPath);
        } catch {
          renderSegments({ language: "zh", segments: [] });
        }
      }
    });
  };

  el<HTMLInputElement>("inputVideo").addEventListener("change", () => {
    fillDerivedPaths();
    setupVideoSource(el<HTMLInputElement>("inputVideo").value.trim());
    setOverlayEnabled(true);
    invoke<string>("suggest_project_path", {
      inputVideo: el<HTMLInputElement>("inputVideo").value.trim() || null,
    })
      .then((path) => {
        if (!el<HTMLInputElement>("projectPath").value.trim()) {
          el<HTMLInputElement>("projectPath").value = path;
        }
      })
      .catch(() => {});
  });

  const onStyleChanged = () => {
    const style = readSubtitleStyle();
    applySubtitleStyleToOverlay(style);
    syncStylePanelPreview(style);
  };
  ["position", "fontSize", "textColor", "bgColor", "roundedRequired", "roundedRadius", "boxPadding", "bgOpacity"].forEach((id) => {
    const node = el<HTMLElement>(id);
    node.addEventListener("input", onStyleChanged);
    node.addEventListener("change", onStyleChanged);
  });
  el<HTMLVideoElement>("previewVideo").addEventListener("click", () => activateTab("subtitles"));

  el<HTMLButtonElement>("btnDetectWhisper").onclick = async () => {
    await step("检测转写环境", async () => {
      const setup = await invoke<LocalSetup>("detect_local_setup");
      applyLocalSetup(setup);
      log(`环境检测: ${setup.message}`);
    });
  };

  el<HTMLButtonElement>("btnInstallWhisper").onclick = async () => {
    await step("安装转写引擎", async () => {
      log("开始安装 whisper-cpp 和 base 模型，可能需要几分钟...");
      const setup = await invoke<LocalSetup>("install_local_whisper_base");
      applyLocalSetup(setup);
      log("安装完成");
    });
  };
}

window.addEventListener("DOMContentLoaded", () => {
  bind();
  activateTab("subtitles");
  renderSegments({ language: "zh", segments: [] });
  const style = readSubtitleStyle();
  applySubtitleStyleToOverlay(style);
  syncStylePanelPreview(style);
  log("应用已启动");
  invoke<string>("suggest_project_path", { inputVideo: null })
    .then((path) => {
      if (!el<HTMLInputElement>("projectPath").value.trim()) {
        el<HTMLInputElement>("projectPath").value = path;
      }
    })
    .catch(() => {});
  invoke<LocalSetup>("detect_local_setup")
    .then((setup) => {
      applyLocalSetup(setup);
      log(`环境检测: ${setup.message}`);
    })
    .catch((e) => log(`环境检测失败: ${errText(e)}`));
});
