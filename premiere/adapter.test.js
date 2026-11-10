const test = require("node:test");
const assert = require("node:assert/strict");
const { submit, result, display } = require("./adapter");
test("request is complete before ready; response reports actual engine state", async () => {
  const files = [];
  const job = {
    async createFile(name) {
      const file = { name, async write(value) { this.value = value; }, async read() { return this.value; } };
      files.push(file);
      return file;
    },
    async getEntries() { return files; }
  };
  await submit(job, { command: "plan" });
  assert.deepEqual(files.map(f => f.name), ["request.json", "ready"]);
  assert.deepEqual(JSON.parse(files[0].value), { command: "plan" });
  assert.equal(await result(job), null);
  const file = await job.createFile("result.json");
  await file.write(JSON.stringify({ status: "full_export_required", reason: "unknown" }));
  assert.match(display(await result(job))[0], /전체 내보내기/);
  assert.match(display({ status: "patch", ranges: [[264, 384]] })[1], /264 ~ 384/);
});
