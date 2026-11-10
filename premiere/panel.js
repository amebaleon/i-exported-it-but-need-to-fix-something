const ppro = require("premierepro");
const { localFileSystem: fs } = require("uxp").storage;
const { submit, result, display } = require("./adapter.js");
const key = "incremental-export-v1";
const status = document.getElementById("status");
const ranges = document.getElementById("ranges");

function state() { return JSON.parse(localStorage.getItem(key) || "{}"); }
function remember(value) { localStorage.setItem(key, JSON.stringify(value)); }
async function active() {
  const project = await ppro.Project.getActiveProject();
  const sequence = project && await project.getActiveSequence();
  if (!sequence) throw new Error("활성 시퀀스가 필요합니다.");
  return { sequence, project_id: project.guid.toString(), sequence_id: sequence.guid.toString() };
}
function newJob(folder) {
  return folder.createFolder(`ix-${Date.now()}-${Math.random().toString(36).slice(2)}`);
}
async function refresh() {
  const s = state();
  if (!s.token || !s.last) throw new Error("기록된 작업이 없습니다.");
  const folder = await fs.getEntryForPersistentToken(s.token);
  const reply = await result(await folder.getEntry(s.last));
  [status.textContent, ranges.textContent] = display(reply);
}
async function first() {
  const folder = await fs.getFolder();
  if (!folder) return;
  const preset = await fs.getFileForOpening({ types: ["epr"] });
  if (!preset) return;
  const { sequence, project_id, sequence_id } = await active();
  const token = await fs.createPersistentToken(folder);
  const job = await newJob(folder);
  const output = fs.getNativePath(job) + "/export.mp4";
  const manager = await ppro.EncoderManager.getManager();
  const accepted = await manager.exportSequence(sequence, ppro.Constants.ExportType.IMMEDIATELY,
    output, fs.getNativePath(preset), true);
  if (!accepted) throw new Error("Premiere에서 내보내기 요청을 거절했습니다.");
  await submit(job, { command: "record", project_id, sequence_id, preset: fs.getNativePath(preset) });
  remember({ token, baseline: job.name, last: job.name });
  await refresh();
}
async function quick() {
  const s = state();
  if (!s.baseline) throw new Error("먼저 첫 내보내기를 실행하세요.");
  const folder = await fs.getEntryForPersistentToken(s.token);
  const cached = await result(await folder.getEntry(s.baseline));
  if (!cached || cached.status !== "recorded") {
    [status.textContent, ranges.textContent] = display(cached);
    return;
  }
  const { project_id, sequence_id } = await active();
  const job = await newJob(folder);
  await submit(job, { command: "plan", project_id, sequence_id, baseline: s.baseline });
  remember({ ...s, last: job.name });
  await refresh();
}
const actions = { first, quick, refresh };
for (const [id, action] of Object.entries(actions)) {
  document.getElementById(id).addEventListener("click", async () => {
    const buttons = Object.keys(actions).map(k => document.getElementById(k));
    buttons.forEach(b => { b.disabled = true; });
    try { ranges.textContent = ""; await action(); }
    catch (error) { status.textContent = String(error.message || error); }
    finally { buttons.forEach(b => { b.disabled = false; }); }
  });
}
