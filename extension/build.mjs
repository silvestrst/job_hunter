import * as esbuild from "esbuild";

const watch = process.argv.includes("--watch");
const options = {
  entryPoints: ["src/popup.ts", "src/content.ts"],
  bundle: true,
  outdir: "dist",
  format: "iife",
  target: "chrome110",
  logLevel: "info",
};

if (watch) {
  const ctx = await esbuild.context(options);
  await ctx.watch();
} else {
  await esbuild.build(options);
}
