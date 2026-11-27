// 构建 `.pez` 曲绘图标处理器（cdylib），并复制到 Tauri 资源目录。
//
// 必须在 `pnpm build` 之前跑：`tauri.conf.json` 的 `bundle.resources` 会把
// `src-tauri/pez-icon/pez_icon.dll` 打进安装包，安装后它与 exe 并排，
// `set_pez_default` 再把这份 DLL 注册成 `.pez` 的 IconHandler。
//
// 用 `stdio: 'inherit'` 而不是管道：本机沙箱下子进程管道会 EPERM。
import { copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const dpr = join(here, '..');
const repo = join(dpr, '..');
const target = process.env.CARGO_TARGET_DIR || join(repo, '..', 'target');
const windows = process.platform === 'win32';

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, stdio: 'inherit', shell: windows });
  if (result.status !== 0) {
    console.error(`[pez-icon] 失败：${command} ${args.join(' ')}（${result.status}）`);
    process.exit(result.status ?? 1);
  }
}

run('cargo', ['build', '-p', 'dpr-pez-icon', '--release'], repo);

const built = join(target, 'release', 'dpr_pez_icon.dll');
if (!existsSync(built)) {
  console.error(`[pez-icon] 找不到 ${built}`);
  process.exit(1);
}
const dest = join(dpr, 'src-tauri', 'pez-icon', 'pez_icon.dll');
mkdirSync(dirname(dest), { recursive: true });
copyFileSync(built, dest);

run('pnpm', ['build'], dpr);
