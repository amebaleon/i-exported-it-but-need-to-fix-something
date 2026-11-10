# 영상내보내기하고고칠거있을때쓰기좋은거

기존 H.264 MP4와 수정 구간을 렌더링한 MP4로 새 영상을 만드는 Windows 우선 Rust CLI입니다. 원본 오디오를 유지하고 변경 밖의 압축 패킷을 복사합니다. Premiere 없이 CLI를 실행할 수 있습니다. Premiere UXP 패널은 실기 검증 전입니다.

## 설치와 첫 실행

실행에는 `incremental-export.exe`, `ffmpeg.exe`, `ffprobe.exe`가 필요합니다. FFmpeg 빌드는 libx264 인코더를 포함해야 합니다. FFmpeg와 FFprobe를 PATH에 두거나 PowerShell 세션에서 두 경로를 지정하세요. 공유 DLL을 쓰는 FFmpeg 빌드는 설치된 bin 폴더의 DLL도 필요합니다.

```powershell
$env:FFMPEG  = 'C:\tools\ffmpeg\bin\ffmpeg.exe'
$env:FFPROBE = 'C:\tools\ffmpeg\bin\ffprobe.exe'
```

소스에서 빌드하려면 Windows용 Rust stable과 MSVC C++ 빌드 도구를 설치하고 저장소 루트에서 실행하세요. 이 프로젝트에서 확인한 컴파일러는 Rust 1.99.1입니다.

```powershell
cargo build --release --locked
.\target\release\incremental-export.exe patch --original "첫 내보내기.mp4" --replacement "수정 구간.mp4" --start 60 --end 65 --output "수정본.mp4"
```

위 명령은 60초 이상 65초 미만을 교체합니다. `--replacement`는 해당 구간 **전체의 최종 화면**이어야 하며 프레임 수가 정확히 일치해야 합니다. 교체 파일의 오디오는 사용하지 않습니다. 출력에는 새 파일명을 지정하세요. 실패 시 종료 코드가 0이 아니며 이유가 stderr에 표시됩니다. 호환성 오류가 나면 편집기에서 전체를 다시 내보내세요.

- 초 단위 정수, 소수, 분수를 받습니다. 30000/1001 fps의 한 프레임은 `1001/30000`초입니다. 프레임 중간 시각은 거절합니다.
- 여러 수정은 같은 명령에 `--edit 90 92 "두 번째 수정.mp4"`처럼 추가합니다. 구간은 겹칠 수 없습니다.
- 성공하면 프레임 구간 `[start, end)`, 재인코딩 프레임 수, 원본 및 교체본에서 복사한 패킷 수, 시간과 파일 바이트 수가 JSON으로 나옵니다. `verified_reused_packets`는 원본에서 가져온 비디오 패킷 수입니다. 교체본은 `copied_replacement_packets`에 따로 집계합니다.

## 엔진이 처리하는 범위

1. Rust가 FFprobe에서 패킷 위치·크기·SHA-256·PTS·DTS·duration을 읽습니다. CFR의 모든 표시 프레임과 실제 H.264 IDR NAL을 확인합니다.
2. 수정의 영향을 받는 닫힌 GOP를 구합니다. 교체본의 AVC 설정 레코드(SPS/PPS 포함)가 원본과 바이트 단위로 일치하면 교체 패킷도 복사하고 양쪽 GOP의 남은 부분만 인코딩합니다. 다르면 FFmpeg CLI로 해당 구간을 원본에 맞게 렌더링합니다. 렌더링한 설정도 일치해야 합니다.
3. Rust가 단일 `mdat`의 원본 패킷과 교체 패킷을 새 MP4에 기록합니다. 샘플 크기, 키프레임, composition offset과 청크 위치를 갱신하고 오디오 트랙·타임베이스·편집 목록을 유지합니다. 중간 파일은 인코딩한 조각과 게시 전 최종 파일뿐입니다. 출력은 `moov`가 끝에 있는 MP4입니다.
4. FFprobe로 모든 결과 패킷 해시와 시각을 대조하고 FFmpeg로 마지막 프레임까지 디코딩합니다. 통과한 파일만 새 경로에 게시합니다.

| 지원 항목 | 조건 |
|---|---|
| 비디오 | 비분할 MP4, 단일 `mdat`, 하나의 avc1/H.264 비디오 트랙, 4바이트 NAL 길이, 닫힌 IDR GOP |
| 화면·시간 | 프로그레시브 8비트 SDR yuv420p, 정사각 픽셀, CFR, 시작 PTS 0. 원본과 교체본의 해상도·FPS·표시 속성이 일치해야 함 |
| 재인코딩 | libx264 medium, High/Main/Constrained Baseline 후보. 비트스트림 설정을 재현할 수 있어야 함. B-frame 및 순서가 다른 PTS/DTS 검증 포함 |
| 오디오 | 원본 0개 또는 1개 트랙. AAC 48 kHz, priming·패킷 시각·디코딩 PCM 검증 |
| 중단 조건 | VFR, HEVC, 길이 변경, 열린 GOP, 인밴드 파라미터 변경, 복수 샘플 설명, 미지원 MP4 테이블, 전체 GOP가 영향받는 경우 등 |

영상 편집 효과는 교체본 렌더링에 반영되어 있어야 합니다. 오디오 편집, HDR 워크플로, 프래그먼트 MP4, 자막·데이터 트랙이 있는 MP4 컨테이너, 모든 Premiere 하드웨어 인코더 프리셋은 현재 지원 범위 밖입니다. 임의의 H.264 입력에 대한 호환성을 보장하지 않습니다. 장시간 영상은 패킷 목록을 메모리에 보관하고 검증 시 전체 파일을 읽으므로 짧은 영상과 비용이 다릅니다.

## 스냅샷과 캐시

`examples/snapshot.json`은 24 fps, 720프레임 모의 시퀀스입니다. 실제 어댑터는 소스 리비전, 오디오 믹스, 자막 스타일, 프리셋의 변경을 식별할 수 있는 리비전 값을 제공해야 합니다. `frame_local`은 모든 레이어와 렌더링 의존성을 확인한 어댑터만 설정합니다. 시간 효과, 전환, 중첩 시퀀스, 누락된 정보가 있으면 `unknown`을 씁니다.

```powershell
incremental-export record --original original.mp4 --snapshot exported.json --cache baseline.json
incremental-export diff --cache baseline.json --current current.json
```

캐시는 개별 내보내기마다 새 파일로 기록합니다. 캐시에는 스냅샷 버전, 프로젝트·시퀀스 ID, 출력 경로·파일 크기·SHA-256, 실제 스트림 설정과 당시 스냅샷이 들어갑니다. 비교 결과는 `unchanged`, 프레임 구간 목록을 담은 `patch`, 이유를 담은 `full_export_required`입니다. 클립 이동은 이전 위치와 새 위치를 모두 포함합니다. 출력 설정·믹스 변경이나 의존성 불확실성은 전체 내보내기로 처리합니다. Rust `EditorAdapter`와 `MockAdapter`가 비교 결과에 필요한 구간만 요청해 패치하는 경로를 제공합니다.

## Premiere 패널 개발판

1. 신뢰하는 로컬 작업 폴더를 만들고 `incremental-export watch --folder "D:\영상 작업"`을 실행합니다.
2. Premiere 25.6 이상과 UXP Developer Tool이 있는 환경에서 `premiere/manifest.json`을 로드합니다.
3. **첫 내보내기**에서 같은 작업 폴더와 H.264 MP4용 `.epr`를 선택합니다. 패널 코드는 새 작업 폴더에 `export.mp4`를 요청합니다. **캐시 상태 확인**에서 Rust 엔진의 실제 파일 검사 결과를 읽습니다.
4. 현재 **빠른 재내보내기**는 프로젝트·시퀀스를 대조한 후 전체 내보내기 안내를 표시합니다. 실제 Premiere의 클립·자막·효과 수집과 구간 렌더가 검증될 때까지 이 경로를 유지합니다. 수동으로 구간을 렌더링할 수 있으면 Rust CLI를 사용할 수 있습니다.

패널은 사용자가 고른 폴더에 버전이 분리된 작업을 남깁니다. `request.json` 작성 후 `ready`를 만들고, Rust가 `result.json`을 게시합니다. 작업 폴더에는 내보낸 MP4와 캐시가 남습니다. 로컬 소켓, 네트워크 권한, C++ Hybrid Plugin은 필요하지 않습니다. UXP의 파일 선택·지속 토큰과 프로세스 실행 제약을 고려한 설계입니다. [Adobe 파일 접근](https://developer.adobe.com/premiere-pro/uxp/resources/recipes/filesystem-operations/) · [외부 실행 제약](https://developer.adobe.com/uxp/guides/how-to/recipes/external-process/)

실기 확인 항목은 프리셋 선택과 파일 권한, `exportSequence` 반환 시점과 실제 렌더 완료·취소 이벤트, GUID·토큰 복구, 지원 가능한 타임라인 수집, 정확한 In/Out 및 복원, 실제 Premiere 결과의 SPS/PPS 호환성입니다. [EncoderManager](https://developer.adobe.com/premiere-pro/uxp/ppro-reference/classes/encodermanager) · [Premiere UXP](https://developer.adobe.com/premiere-pro/uxp/) · [Adobe 공식 예제](https://github.com/AdobeDocs/uxp-premiere-pro-samples)

## 검증과 성능

```powershell
cargo test --release --locked
cargo test --release --locked --test benchmark -- --ignored --nocapture
cargo clippy --all-targets --locked -- -D warnings
node --test premiere/adapter.test.js
```

Rust 테스트가 임시 폴더에 영상을 생성하고 삭제합니다. 측정값·장비·비교 조건과 다음 개발 우선순위는 [실행 보고서](docs/verification.md)에 있습니다.

## English quick start

Build on Windows with stable Rust, MSVC build tools and `cargo build --release --locked`. For use on another machine, copy the release executable and install a libx264-enabled FFmpeg distribution. Put FFmpeg and FFprobe on PATH, or set the `FFMPEG` and `FFPROBE` environment variables to their executable paths. The engine runs independently of Python, Node.js and Premiere.

```powershell
.\target\release\incremental-export.exe patch --original original.mp4 --replacement replacement.mp4 --start 60 --end 65 --output fixed.mp4
```

Supply a complete same-duration render of the edited interval. Times are in seconds with an exclusive end. The output path must be new. Unsupported inputs fail with a diagnostic. The Adobe panel is a development preview awaiting host testing.

## 라이선스

프로젝트 소스에는 MIT 라이선스가 적용됩니다. Rust 의존성은 각 패키지의 라이선스를 따릅니다. 이 저장소는 FFmpeg 바이너리를 배포하지 않습니다. 테스트 빌드는 `--enable-gpl --enable-version3 --enable-libx264`로 구성되었습니다. FFmpeg를 함께 재배포할 경우 해당 빌드와 연결된 라이브러리의 라이선스 및 소스 제공 조건을 별도로 확인해야 합니다. [FFmpeg 라이선스 안내](https://www.ffmpeg.org/legal.html) · [프로젝트 라이선스](LICENSE)
