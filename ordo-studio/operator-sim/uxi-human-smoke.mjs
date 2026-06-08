#!/usr/bin/env node

const studioUrl = process.env.ORDO_STUDIO_URL ?? "http://127.0.0.1:1420";
const tabs = [
  "Provider",
  "Assistant",
  "Modes",
  "Hooks",
  "Review",
  "Settings",
  "Skills",
  "Plugins",
  "MCP",
  "Automation",
  "Dreaming",
  "Diagnostic",
  "Projects",
  "Docs",
  "Dev Docs",
];

let chromium;
try {
  ({ chromium } = await import("playwright"));
} catch {
  console.error("Playwright is not installed. Run `npm install -D playwright` inside ordo-studio first.");
  process.exit(2);
}

let browser;
try {
  browser = await chromium.launch({ channel: "chrome", headless: true });
} catch {
  browser = await chromium.launch({ headless: true });
}
const page = await browser.newPage({ viewport: { width: 1440, height: 920 } });
const report = {
  studioUrl,
  startedAt: new Date().toISOString(),
  finishedAt: null,
  steps: [],
};

async function step(name, run) {
  const started = Date.now();
  try {
    const detail = await run();
    report.steps.push({
      name,
      status: "passed",
      durationMs: Date.now() - started,
      detail: detail ?? null,
    });
  } catch (error) {
    report.steps.push({
      name,
      status: "failed",
      durationMs: Date.now() - started,
      detail: String(error?.stack ?? error),
    });
  }
}

await step("open studio", async () => {
  await page.goto(studioUrl, { waitUntil: "networkidle", timeout: 30_000 });
  await page.screenshot({ path: "operator-sim-open.png", fullPage: true });
  return { title: await page.title() };
});

for (const tab of tabs) {
  await step(`open ${tab}`, async () => {
    const item = page.getByText(tab, { exact: true }).first();
    await item.click({ timeout: 5_000 });
    await page.waitForTimeout(250);
    const bodyBox = await page.locator("body").boundingBox();
    return { bodyBox };
  });
}

await step("assistant chat box visible", async () => {
  await page.getByText("Assistant", { exact: true }).first().click();
  const input = page.getByPlaceholder(/Tell Ordo the brief/i);
  await input.waitFor({ state: "visible", timeout: 5_000 });
  return { visible: true };
});

await step("provider exposes ollama cloud api key setup", async () => {
  await page.getByText("Provider", { exact: true }).first().click();
  await page.getByText("Ollama Cloud API", { exact: true }).first().waitFor({ state: "visible", timeout: 5_000 });
  await page.getByText("Install OLLAMA_API_KEY", { exact: true }).first().waitFor({ state: "visible", timeout: 5_000 });
  return { visible: true };
});

report.finishedAt = new Date().toISOString();
await browser.close();

const failed = report.steps.filter((entry) => entry.status === "failed");
console.log(JSON.stringify(report, null, 2));
process.exit(failed.length === 0 ? 0 : 1);
