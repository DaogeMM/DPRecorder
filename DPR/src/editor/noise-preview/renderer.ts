import blockNoiseUrl from '@repo-assets/blockarea/BlockNoise1.png';
import noiseUrl from '@repo-assets/blockarea/FD_Noise_00000.png';
import pointNoiseUrl from '@repo-assets/blockarea/PointNoise.png';

import { FRAG_BLIT, FRAG_SRC, VERT_BLIT, VERT_BLOCK } from './shaders';

/**
 * 右栏预览用的小型噪域渲染器。
 *
 * 着色器与遮罩管线照 `E:\DPTool\vue` 的噪域画笔（移植自 Phira-Pro，GPL-3.0）：
 * 把文字栅格当成画笔涂过的格子，算出「合成 / 边缘 / 外发光」三张遮罩，
 * 再用引擎同款片元着色器画一遍，所以纹理、噪声、火花的样子和模拟器一致。
 *
 * 和画笔的差别只有三点：格子来自文字、没有交互编辑、材质参数跟着特效轨走。
 */

type GL = WebGLRenderingContext | WebGL2RenderingContext;

const BASE_FLOATS: Record<string, number> = {
  _EdgeOpacity: 0.8,
  _FillStrength: 0.667,
  _FillOpacity: 0.667,
  _GlowIntensity: 0.8,
  _SparkMapOpacity: 5.69,
  _SparkHueShiftAmount: 0.2,
  _SparkDisplaceIntensity: 2.39,
  _DisplaceBlendIntensity: 0.411,
  _DisplaceSpeed: 1.5,
  _DisplaceStrength: 0.15,
  _TouchPosShine: 0,
  _TouchPosRadius: 0.5,
  _TouchPosSDFSmoothness: 0.47,
  _TouchPosSDFFalloff: 0.41,
  _BackgroundPixelScale: 6,
  uDisabledFillOpacity: 0.4,
  uDisabledSparkOpacity: 3.5,
  uDisabledSparkIntensity: 2.29,
  uDisabledSpeed: 0.3,
  _ShineSpeed: 37.9,
  _ShineBrightness: 0.12,
  _TouchDisplaceSpeed: 2.9,
  _TouchDisplaceStrength: 0.08,
  _NoiseEvoSpeed: 0.03,
  _NoiseDirChangeSpeed: 60,
  _NoiseDisplaceStrength: 1,
  _NoiseRadius: 0.48,
  _NoiseSmoothness: 1,
  _SDFCellSize: 0.11,
  _SDFSmoothness: 0.63,
  _SDFFalloff: 0.34,
  _SDFMoveSpeed: 9.3,
  _TouchBackgroundPixelScale: 8,
};

/** 预览一帧需要的全部输入：文字栅格 + 这条特效轨的材质。 */
export interface NoisePreviewFrame {
  /** `gw * gh` 个格子，非 0 表示这里有噪域。 */
  cells: Uint8Array;
  gw: number;
  gh: number;
  /** 动画时间（秒），「流动速度」会缩放它。 */
  time: number;
  /** 噪域颜色，0~255。 */
  color: [number, number, number];
  /** 边缘破碎：0~4，默认 1。 */
  edge: number;
  /** 颗粒大小：像素块边长，默认 6。 */
  pixelScale: number;
  /** 扭曲强度：0~1，默认 0.15。 */
  displace: number;
  /** 噪声半径与平滑度（贴图采样尺度）。 */
  noiseRadius: number;
  noiseSmoothness: number;
  blotch: boolean;
  spark: boolean;
  distort: boolean;
}

const clamp = (value: number, low: number, high: number) => (value < low ? low : value > high ? high : value);

/** 与引擎的 `noise_index` 一致：镜像寻址。 */
function noiseIndex(value: number, size: number): number {
  let v = value % 2;
  if (v < 0) v += 2;
  if (v > 1) v = 2 - v;
  const i = Math.floor(v * size);
  return i < 0 ? 0 : i > size - 1 ? size - 1 : i;
}

/** 256 级噪声查表：中心化并压到半精度，和着色器的 mediump 对齐。 */
const CENTERED = (() => {
  const table = new Float32Array(256);
  for (let i = 0; i < 256; i++) table[i] = i / 255 - 0.5;
  return table;
})();

/** 外发光六圈的权重，系数照上游。 */
const GLOW_W = (() => {
  let sum = 0;
  for (let k = 1; k <= 6; k++) sum += Math.pow(k, 2.65);
  return Array.from({ length: 6 }, (_, pass) => Math.pow(6 - pass, 2.65) / sum);
})();

const MASK_MAX = 1024;

interface MaskTexture {
  rgba: Uint8Array;
  ew: number;
  eh: number;
}

export class NoisePreviewRenderer {
  private gl: GL | null = null;
  private progBlock: WebGLProgram | null = null;
  private progBlit: WebGLProgram | null = null;
  private texDisplace: WebGLTexture | null = null;
  private texSpark: WebGLTexture | null = null;
  private texNoise: WebGLTexture | null = null;
  private texMask: WebGLTexture | null = null;
  private texAux: WebGLTexture | null = null;
  private texScene: WebGLTexture | null = null;
  private noiseR: Uint8Array | null = null;
  private noiseWidth = 0;
  private noiseHeight = 0;
  private uniformCache = new Map<string, WebGLUniformLocation | null>();
  private mask: MaskTexture | null = null;
  private maskKey = '';
  private canvas: HTMLCanvasElement | null = null;
  /** 预载三张贴图并建好 GL 状态。失败时返回 false，调用方退回 Canvas 画法。 */
  async attach(canvas: HTMLCanvasElement): Promise<boolean> {
    this.canvas = canvas;
    try {
      const gl = canvas.getContext('webgl', { alpha: false, antialias: false, depth: false, stencil: false, premultipliedAlpha: false }) as GL | null;
      if (!gl) return false;
      this.gl = gl;

      const [displace, spark, noise] = await Promise.all([loadImage(blockNoiseUrl), loadImage(pointNoiseUrl), loadImage(noiseUrl)]);
      this.noiseWidth = displace.naturalWidth;
      this.noiseHeight = displace.naturalHeight;
      const reader = document.createElement('canvas');
      reader.width = this.noiseWidth;
      reader.height = this.noiseHeight;
      const readerCtx = reader.getContext('2d', { willReadFrequently: true });
      if (!readerCtx) return false;
      readerCtx.drawImage(displace, 0, 0);
      const pixels = readerCtx.getImageData(0, 0, this.noiseWidth, this.noiseHeight).data;
      this.noiseR = new Uint8Array(this.noiseWidth * this.noiseHeight);
      for (let i = 0; i < this.noiseR.length; i++) this.noiseR[i] = pixels[i * 4] ?? 0;

      this.progBlit = link(gl, compile(gl, gl.VERTEX_SHADER, VERT_BLIT), compile(gl, gl.FRAGMENT_SHADER, FRAG_BLIT));
      this.progBlock = link(gl, compile(gl, gl.VERTEX_SHADER, VERT_BLOCK), compile(gl, gl.FRAGMENT_SHADER, FRAG_SRC));

      const vbo = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
      gl.enableVertexAttribArray(0);
      gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

      this.texDisplace = texture(gl, displace, gl.MIRRORED_REPEAT);
      this.texSpark = texture(gl, spark, gl.REPEAT);
      this.texNoise = texture(gl, noise, gl.MIRRORED_REPEAT);
      this.texAux = solid(gl, 0, 0, 0, 0);
      this.texMask = solid(gl, 0, 0, 0, 0);
      this.texScene = solid(gl, 0, 0, 0, 255);
      return true;
    } catch {
      this.dispose();
      return false;
    }
  }

  /** 画一帧。`frame` 里的参数变了就会重算遮罩。 */
  render(frame: NoisePreviewFrame): void {
    const gl = this.gl;
    const canvas = this.canvas;
    if (!gl || !canvas || !this.progBlock || !this.progBlit) return;

    const width = canvas.width;
    const height = canvas.height;
    if (width <= 0 || height <= 0) return;

    this.ensureMask(frame);

    gl.viewport(0, 0, width, height);
    gl.disable(gl.BLEND);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);

    gl.useProgram(this.progBlit);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.texScene);
    gl.uniform1i(this.uniform(gl, this.progBlit, 'uTex', true), 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);

    gl.useProgram(this.progBlock);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

    const u = (name: string) => this.uniform(gl, this.progBlock!, name, false);
    for (const [name, value] of Object.entries(BASE_FLOATS)) gl.uniform1f(u(name), value);

    const [r, g, b] = frame.color.map((value) => value / 255) as [number, number, number];
    // 引擎的 `block_area_normal_color` 会经色相旋转落到三处颜色上；这里直接按颜色缩放，
    // 至少「换成绿色就是绿色」这一点和引擎一致。
    gl.uniform4f(u('_EdgeColor'), r, g, b, 1);
    gl.uniform4f(u('_FillColor'), r * 0.713, g * 0.713, b * 0.713, 1);
    gl.uniform4f(u('_GlowColor'), r, g, b, 1);
    gl.uniform4f(u('uDisabledFillColor'), r * 0.497, g * 0.497, b * 0.497, 1);
    gl.uniform4f(u('_NoiseTint'), r, g, b, 1);
    gl.uniform4f(u('_TouchGlowColor'), r, g, b, 1);
    gl.uniform4f(u('_DisplaceDirection'), 1, 1, 0, 0);
    gl.uniform4f(u('_TouchDisplaceDirection'), 1, 1, 0, 0);
    gl.uniform4f(u('_ShineColor'), 1, 1, 1, 1);
    const sparkPeak = Math.max(r, g, b) || 1;
    gl.uniform3f(u('_SparkTint'), 1, g / sparkPeak, b / sparkPeak);
    gl.uniform3f(u('uDisabledSparkTint'), 0.31132078, 0.077830195, 0.077830195);

    gl.uniform4f(u('uUnityTime'), frame.time / 20, frame.time, frame.time * 2, frame.time * 3);
    gl.uniform4f(u('_ScreenParams'), width, height, 1 + 1 / width, 1 + 1 / height);
    const mask = this.mask;
    if (mask) gl.uniform4f(u('_EffectRT_TexelSize'), 1 / mask.ew, 1 / mask.eh, mask.ew, mask.eh);
    gl.uniform1i(u('_TouchPosCount'), 0);
    gl.uniform1i(u('uLayer'), 3);
    gl.uniform1f(u('uClipHalfWidth'), 1);
    gl.uniform1f(u('_DisplaceStrength'), frame.distort ? frame.displace : 0);
    gl.uniform1f(u('_DisplaceBlendIntensity'), frame.blotch ? 0.411 : 0);
    gl.uniform1f(u('_BackgroundPixelScale'), Math.max(0.02, frame.pixelScale));
    gl.uniform1f(u('_SparkMapOpacity'), frame.spark ? 5.69 : 0);
    gl.uniform1f(u('_NoiseRadius'), frame.noiseRadius);
    gl.uniform1f(u('_NoiseSmoothness'), frame.noiseSmoothness);
    gl.uniform1f(u('_DisplaceSpeed'), BASE_FLOATS['_DisplaceSpeed'] ?? 1.5);

    bind(gl, 0, this.texDisplace, u('uDisplaceTex'));
    bind(gl, 1, this.texSpark, u('uSparkTex'));
    bind(gl, 2, this.texMask, u('uMasks'));
    bind(gl, 3, this.texScene, u('uScene'));
    bind(gl, 4, this.texAux, u('uAuxMasks'));
    bind(gl, 5, this.texNoise, u('uNoiseTex'));

    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    gl.disable(gl.BLEND);
  }

  dispose(): void {
    const gl = this.gl;
    if (gl) {
      const lose = gl.getExtension('WEBGL_lose_context');
      if (lose) lose.loseContext();
    }
    this.gl = null;
    this.progBlock = null;
    this.progBlit = null;
    this.texDisplace = this.texSpark = this.texNoise = this.texMask = this.texAux = this.texScene = null;
    this.mask = null;
    this.maskKey = '';
    this.uniformCache.clear();
  }

  private uniform(gl: GL, program: WebGLProgram, name: string, blit: boolean): WebGLUniformLocation | null {
    const key = (blit ? 'blit:' : 'block:') + name;
    if (!this.uniformCache.has(key)) this.uniformCache.set(key, gl.getUniformLocation(program, name));
    return this.uniformCache.get(key) ?? null;
  }

  /** 遮罩只在文字、边缘破碎、颗粒大小变化时重算；时间按 0.1 秒量化，动画时不用每帧重算。 */
  private ensureMask(frame: NoisePreviewFrame): void {
    const gl = this.gl;
    if (!gl || !this.texMask) return;
    const key = maskKeyOf(frame);
    if (key === this.maskKey && this.mask) return;
    const mask = buildMask(frame, this.noiseR, this.noiseWidth, this.noiseHeight);
    this.mask = mask;
    this.maskKey = key;
    gl.bindTexture(gl.TEXTURE_2D, this.texMask);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, mask.ew, mask.eh, 0, gl.RGBA, gl.UNSIGNED_BYTE, mask.rgba);
  }
}

/** 遮罩指纹：文字格子抽一份摘要，再带上只影响遮罩的几个参数。 */
function maskKeyOf(frame: NoisePreviewFrame): string {
  let hash = 2166136261;
  for (let i = 0; i < frame.cells.length; i += 5) hash = ((hash ^ (frame.cells[i] ?? 0)) * 16777619) >>> 0;
  return `${frame.gw}x${frame.gh}|${hash}|${Math.round(frame.edge * 100)}|${Math.round(frame.pixelScale * 10)}|${Math.round(frame.time * 10)}`;
}

function buildMask(frame: NoisePreviewFrame, noiseR: Uint8Array | null, noiseWidth: number, noiseHeight: number): MaskTexture {
  // 颗粒大小：每格再并成 `down × down` 的块（默认 6 时保持文字原来的细粒度）
  const down = clamp(Math.round(frame.pixelScale / 6), 1, 16);
  const bw = Math.max(1, Math.min(Math.floor(frame.gw / down), MASK_MAX));
  const bh = Math.max(1, Math.min(Math.floor(frame.gh / down), MASK_MAX));
  const cells = new Uint8Array(bw * bh);
  for (let y = 0; y < bh; y++) {
    for (let x = 0; x < bw; x++) {
      let on = false;
      for (let dy = 0; dy < down && !on; dy++) {
        const cy = y * down + dy;
        if (cy >= frame.gh) break;
        const row = cy * frame.gw;
        for (let dx = 0; dx < down; dx++) {
          const cx = x * down + dx;
          if (cx >= frame.gw) break;
          if (frame.cells[row + cx]) {
            on = true;
            break;
          }
        }
      }
      cells[y * bw + x] = on ? 255 : 0;
    }
  }

  // 边缘破碎：每个格子按远处噪声决定归属，外轮廓从一条折线碎成一颗颗小方块
  const amp = 0.1 * Math.max(0, frame.edge);
  if (amp <= 0 || !noiseR || noiseWidth <= 0 || noiseHeight <= 0) return upsample(cells, bw, bh);

  const d = 0.70703125;
  const dt = d * ((frame.time / 20) * 2.59);
  const wxA = new Int32Array(bw);
  const wxB = new Int32Array(bw);
  for (let x = 0; x < bw; x++) {
    const u = (x + 0.5) / bw;
    wxA[x] = noiseIndex(dt + u * 2.13, noiseWidth);
    wxB[x] = noiseIndex(-dt + u * 2.13, noiseWidth);
  }
  const wy = new Int32Array(bh);
  for (let y = 0; y < bh; y++) wy[y] = noiseIndex(dt + ((y + 0.5) / bh) * 1.02, noiseHeight) * noiseWidth;
  const shifted = new Uint8Array(bw * bh);
  for (let y = 0; y < bh; y++) {
    const row = wy[y] ?? 0;
    for (let x = 0; x < bw; x++) {
      const a = CENTERED[noiseR[row + (wxA[x] ?? 0)] ?? 0] ?? 0;
      const b = CENTERED[noiseR[row + (wxB[x] ?? 0)] ?? 0] ?? 0;
      const u = (d * a + b * -d) * amp + (x + 0.5) / bw;
      const v = (d * a + b * d) * amp + (y + 0.5) / bh;
      const sx = clamp(Math.floor(u * bw), 0, bw - 1);
      const sy = clamp(Math.floor(v * bh), 0, bh - 1);
      shifted[y * bw + x] = cells[sy * bw + sx] ?? 0;
    }
  }
  return upsample(shifted, bw, bh);
}

/** 2 倍最近邻上采样，同时算边缘环带与外发光两圈遮罩。 */
function upsample(composed: Uint8Array, bw: number, bh: number): MaskTexture {
  const ew = bw * 2;
  const eh = bh * 2;
  const rgba = new Uint8Array(ew * eh * 4);
  const stride = (ew + 31) >> 5;
  const active = new Uint32Array(stride * eh);
  for (let y = 0; y < bh; y++) {
    for (let x = 0; x < bw; x++) {
      const value = composed[y * bw + x];
      if (!value) continue;
      const px = x * 2;
      const py = y * 2;
      const o = (py * ew + px) * 4;
      const o2 = o + ew * 4;
      rgba[o] = value;
      rgba[o + 4] = value;
      rgba[o2] = value;
      rgba[o2 + 4] = value;
      const wi = px >> 5;
      const shift = px & 31;
      const bits = (3 << shift) >>> 0;
      active[py * stride + wi] = ((active[py * stride + wi] ?? 0) | bits) >>> 0;
      active[(py + 1) * stride + wi] = ((active[(py + 1) * stride + wi] ?? 0) | bits) >>> 0;
    }
  }
  // 官方遮罩的边缘环带就是一圈 2× 过采样格（半格像素），再往外算外发光
  const passes = 1;
  let ping = active;
  let pong = new Uint32Array(active.length);
  for (let pass = 0; pass < 6; pass++) {
    const weight = GLOW_W[pass] ?? 0;
    if (weight < 0.01) break;
    dilate32(ping, pong, ew, eh);
    for (let y = 0; y < eh; y++) {
      const base = y * stride;
      for (let wi = 0; wi < stride; wi++) {
        let ring = ((pong[base + wi] ?? 0) & ~(ping[base + wi] ?? 0)) >>> 0;
        while (ring !== 0) {
          const low = (ring & -ring) >>> 0;
          const bit = 31 - Math.clz32(low);
          const x = wi * 32 + bit;
          if (x < ew) {
            const dst = (y * ew + x) * 4;
            if (pass < passes) rgba[dst + 1] = 255;
            rgba[dst + 2] = Math.round(weight * 255);
          }
          ring = (ring & (ring - 1)) >>> 0;
        }
      }
    }
    const swap = ping;
    ping = pong;
    pong = swap;
  }
  return { rgba, ew, eh };
}

/** 九宫格膨胀，位图按行打包成 32 位整数。 */
function dilate32(src: Uint32Array, dst: Uint32Array, width: number, height: number): void {
  const stride = (width + 31) >> 5;
  const tail = (0xffffffff >>> ((32 - (width % 32)) % 32)) >>> 0;
  for (let y = 0; y < height; y++) {
    const y0 = y > 0 ? y - 1 : 0;
    const y1 = y < height - 1 ? y + 1 : height - 1;
    for (let w = 0; w < stride; w++) {
      let value = 0;
      for (let row = y0; row <= y1; row++) {
        const index = row * stride + w;
        const mid = src[index] ?? 0;
        const left = w > 0 ? (src[index - 1] ?? 0) >>> 31 : 0;
        const right = w + 1 < stride ? (src[index + 1] ?? 0) << 31 : 0;
        value |= mid | (mid << 1) | left | (mid >>> 1) | right;
      }
      dst[y * stride + w] = w + 1 === stride ? (value & tail) >>> 0 : value >>> 0;
    }
  }
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error('贴图载入失败'));
    image.src = src;
  });
}

function compile(gl: GL, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type)!;
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`着色器编译失败：${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

function link(gl: GL, vertex: WebGLShader, fragment: WebGLShader): WebGLProgram {
  const program = gl.createProgram()!;
  gl.attachShader(program, vertex);
  gl.attachShader(program, fragment);
  gl.bindAttribLocation(program, 0, 'aPos');
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(`着色器链接失败：${gl.getProgramInfoLog(program)}`);
  }
  return program;
}

function texture(gl: GL, image: HTMLImageElement, wrap: number): WebGLTexture {
  const result = gl.createTexture()!;
  gl.bindTexture(gl.TEXTURE_2D, result);
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, wrap);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, wrap);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  return result;
}

function solid(gl: GL, r: number, g: number, b: number, a: number): WebGLTexture {
  const result = gl.createTexture()!;
  gl.bindTexture(gl.TEXTURE_2D, result);
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([r, g, b, a]));
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return result;
}

function bind(gl: GL, unit: number, tex: WebGLTexture | null, location: WebGLUniformLocation | null): void {
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, tex);
  gl.uniform1i(location, unit);
}
