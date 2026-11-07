// 把 wasm-bindgen 0.2.129 生成的 JS 改成「实例化交给谁、env 从哪来」都能配合 miniquad 胶水的样子。
//
// 背景：miniquad 的 wasm 后端向 host 要 86 个 `env` 函数（canvas_width / glGenBuffers / ...），
// 这些函数由 miniquad 自带的 JS 胶水（gl.js）里的 `importObject.env` 提供。wasm-bindgen 看到
// 这些「自己不认识的环境导入」后，会给每一个都生成一条 `import * as importN from "env"`，
// 浏览器里根本没有名为 "env" 的 ES 模块，页面会直接 SyntaxError。
//
// 所以这里做两件事：
//   1. 删掉那些 `import * as importN from "env"` 行；
//   2. 把 `__wbg_get_imports()` 的返回表改成只给 bindgen 自己的模块名 + 从 `globalThis.__mq_env`
//      取 env（页面在调用 init() 之前把 gl.js 的 importObject.env 挂上去）。
//
// 用法: node patch_bindgen.mjs <生成的 js 路径> <out-name>

import fs from "node:fs";

const [, , jsPath, outName] = process.argv;
if (!jsPath || !outName) {
    console.error("usage: node patch_bindgen.mjs <bindgen.js> <out-name>");
    process.exit(2);
}

let lines = fs.readFileSync(jsPath, "utf8").split("\n");

const before = lines.length;
lines = lines.filter((l) => !/^import \* as import\d+ from "env"\s*$/.test(l));
const removedImports = before - lines.length;

// 定位 __wbg_get_imports() 函数体
const fnStart = lines.findIndex((l) => l.startsWith("function __wbg_get_imports()"));
if (fnStart < 0) throw new Error("没找到 function __wbg_get_imports()");
let fnEnd = -1;
for (let i = fnStart + 1; i < lines.length; i++) {
    if (lines[i] === "}") { fnEnd = i; break; }
}
if (fnEnd < 0) throw new Error("没找到 __wbg_get_imports() 的结尾");

const retStart = lines.findIndex((l, i) => i > fnStart && i < fnEnd && /^\s{4}return \{$/.test(l));
if (retStart < 0) throw new Error("没找到 __wbg_get_imports() 里的 return {");
let retEnd = -1;
for (let i = retStart + 1; i < fnEnd; i++) {
    if (/^\s{4}\};$/.test(lines[i])) { retEnd = i; break; }
}
if (retEnd < 0) throw new Error("没找到 return { ... } 的结尾");

const newReturn = [
    "    return {",
    "        __proto__: null,",
    `        "./${outName}_bg.js": import0,`,
    "        // miniquad 的环境函数由 gl.js 提供，页面在 init() 之前把 importObject.env 挂到这里。",
    "        \"env\": globalThis.__mq_env ?? {},",
    "    };",
];

lines = [...lines.slice(0, retStart), ...newReturn, ...lines.slice(retEnd + 1)];

// __wbg_finalize_init() 末尾会直接调 __wbindgen_start()，而 rust 的 main() 就是从那里起来的；
// miniquad 的 JS 胶水（gl.js）在 main 里就要用 wasm_memory 取 wasm 内存，所以必须在调用之前填好。
const startCall = lines.findIndex((l) => l.includes("wasm.__wbindgen_start();"));
if (startCall < 0) throw new Error("没找到 wasm.__wbindgen_start();");
lines.splice(
    startCall,
    0,
    "    // miniquad 的 JS 胶水（gl.js）靠这两个全局量拿 wasm 内存与导出，而 main() 由下一行拉起。",
    "    globalThis.wasm_memory = wasm.memory;",
    "    globalThis.wasm_exports = wasm;",
);

// import0 是 bindgen 自己的模块（合法），import1..importN 是那些要删掉的 env 伪模块
const leftover = lines.filter((l) => /\bimport[1-9]\d*\b/.test(l));
if (leftover.length) throw new Error(`还残留对 importN 的引用:\n${leftover.join("\n")}`);

fs.writeFileSync(jsPath, lines.join("\n"));
console.log(`[patch] 删掉 ${removedImports} 行 "env" ES 导入，__wbg_get_imports() 改为读 globalThis.__mq_env`);
