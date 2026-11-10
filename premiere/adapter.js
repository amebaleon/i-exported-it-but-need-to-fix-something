// File mailbox shared with the Rust "watch" command. The ready marker is created last.
async function submit(job, request) {
  const file = await job.createFile("request.json", { overwrite: false });
  await file.write(JSON.stringify(request));
  await job.createFile("ready", { overwrite: false });
}
async function result(job) {
  const files = await job.getEntries();
  const response = files.find(entry => entry.name === "result.json");
  return response ? JSON.parse(await response.read()) : null;
}
function display(reply) {
  if (!reply) return ["엔진 응답 대기 중입니다. 작업 폴더에서 Rust 엔진의 watch 명령을 실행한 뒤 캐시 상태를 확인하세요.", ""];
  switch (reply.status) {
    case "recorded": return [`캐시 기록 완료 · ${reply.frames}프레임\nPremiere 타임라인의 효과·자막 추적은 실기 검증 대기 중입니다.`, ""];
    case "full_export_required": return [`전체 내보내기가 필요합니다.\n${reply.reason}`, ""];
    case "patch": return ["변경 구간을 확인했습니다.", reply.ranges.map(([a, b]) => `프레임 ${a} ~ ${b} (끝 제외)`).join("\n")];
    case "unchanged": return ["변경 사항이 없습니다.", ""];
    case "error": return [`엔진 오류 · ${reply.message}`, ""];
    default: throw new Error("알 수 없는 엔진 응답입니다.");
  }
}
module.exports = { submit, result, display };
