// File path → Shiki language id. Most extensions are Shiki aliases already;
// this table covers the ones that aren't, plus special file names.

const byName: Record<string, string> = {
  dockerfile: "docker",
  makefile: "make",
  "cmakelists.txt": "cmake",
  "cargo.lock": "toml",
  gemfile: "ruby",
  rakefile: "ruby",
  justfile: "just",
  ".gitignore": "shellscript",
  ".bashrc": "shellscript",
  ".zshrc": "shellscript",
};

const byExt: Record<string, string> = {
  h: "c",
  hh: "cpp",
  hpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  mjs: "javascript",
  cjs: "javascript",
  tsx: "tsx",
  jsx: "jsx",
  yml: "yaml",
  md: "markdown",
  mdx: "mdx",
  sh: "shellscript",
  bash: "shellscript",
  zsh: "shellscript",
  kt: "kotlin",
  kts: "kotlin",
  rs: "rust",
  py: "python",
  rb: "ruby",
  pl: "perl",
  ex: "elixir",
  exs: "elixir",
  hs: "haskell",
  cs: "csharp",
  fs: "fsharp",
  m: "objective-c",
  mm: "objective-cpp",
  gradle: "groovy",
  tf: "hcl",
  proto: "proto",
  svg: "xml",
  plist: "xml",
  lock: "json",
};

/** Resolves a language id, given the set Shiki knows (ids and aliases). */
export function langForPath(path: string, known: (id: string) => boolean): string | null {
  const file = path.split("/").pop()!.toLowerCase();
  if (byName[file]) return byName[file];
  const dot = file.lastIndexOf(".");
  if (dot < 0) return null;
  const ext = file.slice(dot + 1);
  const mapped = byExt[ext] ?? ext;
  return known(mapped) ? mapped : null;
}
