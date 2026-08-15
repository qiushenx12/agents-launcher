from __future__ import annotations

import copy
import filecmp
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime
from pathlib import Path
from typing import Any


PROJECT_DIR = Path(__file__).resolve().parent
VERSION_FILE = PROJECT_DIR / "version.json"
DEFAULT_VERSION = "1.0.0"
VERSION_PATTERN = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
SUPPORTED_PLATFORMS = ("windows", "macos")
PLATFORM_LABELS = {
    "windows": "Windows",
    "macos": "macOS",
}


class VersionStateError(ValueError):
    pass


def atomic_write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="",
            dir=path.parent,
            prefix=f".{path.name}.",
            suffix=".tmp",
            delete=False,
        ) as temp_file:
            temp_file.write(content)
            temp_path = Path(temp_file.name)
        os.replace(temp_path, path)
    finally:
        if temp_path is not None and temp_path.exists():
            temp_path.unlink()


def read_json(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise VersionStateError(f"文件不存在：{path}") from error
    except json.JSONDecodeError as error:
        raise VersionStateError(f"JSON 格式无效：{path}（{error}）") from error
    if not isinstance(data, dict):
        raise VersionStateError(f"JSON 根节点必须是对象：{path}")
    return data


def write_json(path: Path, data: dict[str, Any]) -> None:
    atomic_write_text(path, json.dumps(data, ensure_ascii=False, indent=2) + "\n")


def parse_version(version: str) -> tuple[int, int, int]:
    match = VERSION_PATTERN.fullmatch(version)
    if match is None:
        raise VersionStateError(f"版本号必须使用 major.minor.patch 格式：{version!r}")
    major, minor, patch = (int(part) for part in match.groups())
    if patch > 9:
        raise VersionStateError(f"修订版本号只能是 0 到 9：{version!r}")
    return major, minor, patch


def next_version(version: str) -> str:
    major, minor, patch = parse_version(version)
    if patch < 9:
        return f"{major}.{minor}.{patch + 1}"
    return f"{major}.{minor + 1}.0"


def pending_platform_record() -> dict[str, Any]:
    return {
        "status": "pending",
        "architecture": None,
        "testedAt": None,
        "artifacts": [],
    }


def new_platform_records() -> dict[str, dict[str, Any]]:
    return {platform_key: pending_platform_record() for platform_key in SUPPORTED_PLATFORMS}


def default_version_state() -> dict[str, Any]:
    return {
        "schemaVersion": 2,
        "currentVersion": DEFAULT_VERSION,
        "published": False,
        "requiredPlatforms": list(SUPPORTED_PLATFORMS),
        "platforms": new_platform_records(),
        "releases": [],
    }


def migrate_version_state(state: dict[str, Any]) -> tuple[dict[str, Any], bool]:
    if state.get("schemaVersion") != 1:
        return state, False
    migrated = copy.deepcopy(state)
    migrated["schemaVersion"] = 2
    migrated["requiredPlatforms"] = list(SUPPORTED_PLATFORMS)
    migrated["platforms"] = new_platform_records()
    migrated.setdefault("releases", [])
    return migrated, True


def validate_artifacts(artifacts: Any, location: str) -> None:
    if not isinstance(artifacts, list):
        raise VersionStateError(f"{location} 的 artifacts 必须是数组")
    for artifact in artifacts:
        if (
            not isinstance(artifact, dict)
            or not isinstance(artifact.get("path"), str)
            or not isinstance(artifact.get("size"), int)
        ):
            raise VersionStateError(f"{location} 中存在无效的安装包记录")


def validate_platform_record(record: Any, platform_key: str) -> None:
    if not isinstance(record, dict):
        raise VersionStateError(f"version.json 缺少 {platform_key} 平台状态")
    if record.get("status") not in {"pending", "passed"}:
        raise VersionStateError(f"{platform_key} 的 status 必须是 pending 或 passed")
    architecture = record.get("architecture")
    if architecture is not None and not isinstance(architecture, str):
        raise VersionStateError(f"{platform_key} 的 architecture 必须是字符串或 null")
    tested_at = record.get("testedAt")
    if tested_at is not None and not isinstance(tested_at, str):
        raise VersionStateError(f"{platform_key} 的 testedAt 必须是字符串或 null")
    validate_artifacts(record.get("artifacts"), f"{platform_key} 平台状态")
    if record["status"] == "passed" and not record["artifacts"]:
        raise VersionStateError(f"{platform_key} 已通过但没有安装包记录")


def validate_version_state(state: dict[str, Any]) -> None:
    if state.get("schemaVersion") != 2:
        raise VersionStateError("version.json 的 schemaVersion 必须为 2")
    current_version = state.get("currentVersion")
    if not isinstance(current_version, str):
        raise VersionStateError("version.json 缺少 currentVersion")
    parse_version(current_version)
    if not isinstance(state.get("published"), bool):
        raise VersionStateError("version.json 的 published 必须是布尔值")

    required_platforms = state.get("requiredPlatforms")
    if not isinstance(required_platforms, list) or not required_platforms:
        raise VersionStateError("version.json 的 requiredPlatforms 必须是非空数组")
    if any(not isinstance(item, str) for item in required_platforms):
        raise VersionStateError("version.json 的 requiredPlatforms 只能包含字符串")
    if len(required_platforms) != len(set(required_platforms)):
        raise VersionStateError("version.json 的 requiredPlatforms 不能重复")
    unsupported = [item for item in required_platforms if item not in SUPPORTED_PLATFORMS]
    if unsupported:
        raise VersionStateError(f"version.json 包含不支持的平台：{', '.join(unsupported)}")

    platforms = state.get("platforms")
    if not isinstance(platforms, dict):
        raise VersionStateError("version.json 的 platforms 必须是对象")
    for platform_key in SUPPORTED_PLATFORMS:
        validate_platform_record(platforms.get(platform_key), platform_key)

    releases = state.get("releases")
    if not isinstance(releases, list):
        raise VersionStateError("version.json 的 releases 必须是数组")
    seen_versions: set[str] = set()
    for release in releases:
        if not isinstance(release, dict) or not isinstance(release.get("version"), str):
            raise VersionStateError("version.json 中存在无效的发布记录")
        release_version = release["version"]
        parse_version(release_version)
        if release_version in seen_versions:
            raise VersionStateError(f"version.json 中存在重复发布版本：{release_version}")
        seen_versions.add(release_version)


def load_version_state(path: Path = VERSION_FILE) -> dict[str, Any]:
    if not path.exists():
        state = default_version_state()
        write_json(path, state)
        return state
    state, migrated = migrate_version_state(read_json(path))
    validate_version_state(state)
    if migrated:
        write_json(path, state)
    return state


def save_version_state(state: dict[str, Any], path: Path = VERSION_FILE) -> None:
    validate_version_state(state)
    write_json(path, state)


def replace_cargo_package_version(path: Path, package_name: str, version: str) -> None:
    content = path.read_text(encoding="utf-8")
    if path.name == "Cargo.toml":
        package_start = content.find("[package]")
        if package_start < 0:
            raise VersionStateError(f"未在 {path} 中找到 [package]")
        next_section = content.find("\n[", package_start + len("[package]"))
        if next_section < 0:
            next_section = len(content)
        package_section = content[package_start:next_section]
        updated_section, count = re.subn(
            r'(?m)^version\s*=\s*"[^"]+"',
            f'version = "{version}"',
            package_section,
            count=1,
        )
        if count != 1:
            raise VersionStateError(f"未在 {path} 的 [package] 中找到版本号")
        updated = content[:package_start] + updated_section + content[next_section:]
    else:
        pattern = re.compile(
            rf'(\[\[package\]\]\s+name\s*=\s*"{re.escape(package_name)}"\s+'
            r'version\s*=\s*")[^"]+("\s*)',
        )
        updated, count = pattern.subn(rf'\g<1>{version}\g<2>', content, count=1)
        if count != 1:
            raise VersionStateError(f"未在 {path} 中找到 {package_name} 的版本号")
    if updated != content:
        atomic_write_text(path, updated)


def sync_project_versions(version: str, project_dir: Path = PROJECT_DIR) -> None:
    parse_version(version)

    package_path = project_dir / "package.json"
    package_data = read_json(package_path)
    package_data["version"] = version
    write_json(package_path, package_data)

    package_lock_path = project_dir / "package-lock.json"
    package_lock_data = read_json(package_lock_path)
    package_lock_data["version"] = version
    root_package = package_lock_data.get("packages", {}).get("")
    if not isinstance(root_package, dict):
        raise VersionStateError("package-lock.json 缺少根包信息")
    root_package["version"] = version
    write_json(package_lock_path, package_lock_data)

    tauri_config_path = project_dir / "src-tauri" / "tauri.conf.json"
    tauri_config = read_json(tauri_config_path)
    tauri_config["version"] = version
    write_json(tauri_config_path, tauri_config)

    replace_cargo_package_version(
        project_dir / "src-tauri" / "Cargo.toml",
        "agents-launcher",
        version,
    )
    replace_cargo_package_version(
        project_dir / "src-tauri" / "Cargo.lock",
        "agents-launcher",
        version,
    )


def reset_platform_records(state: dict[str, Any]) -> None:
    state["platforms"] = new_platform_records()


def prepare_build_version(
    version_file: Path = VERSION_FILE,
    project_dir: Path = PROJECT_DIR,
) -> tuple[str, dict[str, Any]]:
    state = load_version_state(version_file)
    version = state["currentVersion"]
    if state["published"]:
        version = next_version(version)
        state["currentVersion"] = version
        state["published"] = False
        reset_platform_records(state)
        print(f"上一个版本已发布，本次打包版本自动更新为 {version}。")

    sync_project_versions(version, project_dir)
    save_version_state(state, version_file)
    return version, state


def detect_current_platform(system_platform: str = sys.platform) -> str:
    if system_platform == "win32":
        return "windows"
    if system_platform == "darwin":
        return "macos"
    raise VersionStateError(f"当前系统不支持正式打包：{system_platform}")


def normalized_architecture(machine: str | None = None) -> str:
    value = (machine or platform.machine()).lower()
    if value in {"amd64", "x86_64"}:
        return "x64"
    if value in {"arm64", "aarch64"}:
        return "arm64"
    return value or "unknown"


def begin_platform_build(
    state: dict[str, Any],
    platform_key: str,
    version_file: Path = VERSION_FILE,
) -> None:
    if platform_key not in SUPPORTED_PLATFORMS:
        raise VersionStateError(f"不支持的平台：{platform_key}")
    if state["published"]:
        raise VersionStateError("已发布版本不能重新开始平台打包")
    state["platforms"][platform_key] = pending_platform_record()
    save_version_state(state, version_file)


def check_rust() -> bool:
    rustc = shutil.which("rustc")
    cargo = shutil.which("cargo")
    if rustc is None or cargo is None:
        print("未找到 Rust 工具链，请先从 https://rustup.rs 安装。")
        return False
    result = subprocess.run([rustc, "-V"], capture_output=True, text=True)
    if result.returncode != 0:
        print("Rust 工具链检查失败。")
        return False
    print(f"{result.stdout.strip()}：正常")
    return True


def find_npm() -> str | None:
    return shutil.which("npm.cmd") or shutil.which("npm")


def check_npm() -> bool:
    npm = find_npm()
    if npm is None:
        print("未找到 npm，请先安装 Node.js 并添加到 PATH。")
        return False
    result = subprocess.run([npm, "--version"], capture_output=True, text=True)
    if result.returncode != 0:
        print("npm 检查失败。")
        return False
    print(f"npm {result.stdout.strip()}：正常")
    return True


def install_deps() -> bool:
    package_json = PROJECT_DIR / "package.json"
    node_modules = PROJECT_DIR / "node_modules"
    if not package_json.exists():
        print("未找到 package.json。")
        return False
    if node_modules.exists():
        print("npm 依赖已安装。")
        return True

    npm = find_npm()
    if npm is None:
        return False
    print("未找到 node_modules，正在安装 npm 依赖……")
    result = subprocess.run([npm, "install"], cwd=PROJECT_DIR)
    if result.returncode != 0:
        print("npm install 失败。")
        return False
    print("npm 依赖安装完成。")
    return True


def load_product_name(project_dir: Path = PROJECT_DIR) -> str:
    config = read_json(project_dir / "src-tauri" / "tauri.conf.json")
    product_name = config.get("productName")
    if not isinstance(product_name, str) or not product_name.strip():
        raise VersionStateError("tauri.conf.json 缺少 productName")
    return product_name


def platform_bundle_dir(platform_key: str, project_dir: Path = PROJECT_DIR) -> Path:
    bundle_root = project_dir / "src-tauri" / "target" / "release" / "bundle"
    if platform_key == "windows":
        return bundle_root / "nsis"
    if platform_key == "macos":
        return bundle_root / "dmg"
    raise VersionStateError(f"不支持的平台：{platform_key}")


def platform_archive_dir(platform_key: str, project_dir: Path = PROJECT_DIR) -> Path:
    archive_root = project_dir / "src-tauri" / "release-bundle"
    if platform_key == "windows":
        return archive_root / "nsis"
    if platform_key == "macos":
        return archive_root / "dmg"
    raise VersionStateError(f"不支持的平台：{platform_key}")


def artifact_glob(platform_key: str) -> str:
    if platform_key == "windows":
        return "*.exe"
    if platform_key == "macos":
        return "*.dmg"
    raise VersionStateError(f"不支持的平台：{platform_key}")


def is_current_artifact(
    filename: str,
    product_name: str,
    version: str,
    platform_key: str,
) -> bool:
    if not filename.startswith(f"{product_name}_{version}_"):
        return False
    if platform_key == "windows":
        return filename.endswith("-setup.exe")
    if platform_key == "macos":
        return filename.endswith(".dmg")
    return False


def restore_archived_artifacts(archive_dir: Path, bundle_dir: Path, pattern: str) -> int:
    if not archive_dir.exists():
        return 0
    bundle_dir.mkdir(parents=True, exist_ok=True)
    restored = 0
    for archived_artifact in archive_dir.glob(pattern):
        target = bundle_dir / archived_artifact.name
        if not target.exists() or not filecmp.cmp(archived_artifact, target, shallow=False):
            shutil.copy2(archived_artifact, target)
            restored += 1
    return restored


def archive_artifacts(
    artifacts: list[Path],
    archive_dir: Path,
    allow_replace: bool = False,
) -> list[Path]:
    archive_dir.mkdir(parents=True, exist_ok=True)
    archived_paths: list[Path] = []
    for artifact in artifacts:
        archived_path = archive_dir / artifact.name
        if archived_path.exists() and not filecmp.cmp(artifact, archived_path, shallow=False):
            if not allow_replace:
                raise VersionStateError(
                    f"历史安装包已存在且内容不同，拒绝覆盖：{archived_path}"
                )
            shutil.copy2(artifact, archived_path)
        elif not archived_path.exists():
            shutil.copy2(artifact, archived_path)
        archived_paths.append(archived_path)
    return archived_paths


def platform_build_command(npm: str, platform_key: str) -> list[str]:
    command = [npm, "run", "tauri", "build"]
    if platform_key == "macos":
        command.extend(["--", "--bundles", "app,dmg"])
    return command


def run_build(version: str, product_name: str, platform_key: str) -> bool:
    npm = find_npm()
    if npm is None:
        return False

    bundle_dir = platform_bundle_dir(platform_key)
    archive_dir = platform_archive_dir(platform_key)
    pattern = artifact_glob(platform_key)
    archived_restored = restore_archived_artifacts(archive_dir, bundle_dir, pattern)
    if archived_restored:
        print(f"已从发布归档恢复 {archived_restored} 个历史安装包。")
    bundle_dir.mkdir(parents=True, exist_ok=True)
    existing_artifacts = list(bundle_dir.glob(pattern))
    platform_label = PLATFORM_LABELS[platform_key]
    print(f"正在打包 Agents Launcher {version}（{platform_label}）……")

    with tempfile.TemporaryDirectory(prefix="agents-launcher-installer-history-") as backup_dir:
        backup_path = Path(backup_dir)
        for artifact in existing_artifacts:
            shutil.copy2(artifact, backup_path / artifact.name)

        build_env = os.environ.copy()
        build_env.pop("CODEX_PROXY_DEBUG_COMPACT", None)
        build_env.pop("CODEX_PROXY_DEBUG_COMPACT_LOG", None)
        result = subprocess.run(
            platform_build_command(npm, platform_key),
            cwd=PROJECT_DIR,
            env=build_env,
        )

        restored = 0
        for artifact in existing_artifacts:
            if is_current_artifact(artifact.name, product_name, version, platform_key):
                if result.returncode != 0 and not artifact.exists():
                    shutil.copy2(backup_path / artifact.name, artifact)
                    restored += 1
                continue
            shutil.copy2(backup_path / artifact.name, artifact)
            restored += 1

    if restored:
        print(f"已保留 {restored} 个历史安装包。")
    if result.returncode != 0:
        print(
            f"\n{platform_label} 版本 {version} 打包失败；"
            "该平台保持待测试，版本号不会递增。"
        )
        return False
    return True


def find_built_artifacts(
    version: str,
    product_name: str,
    platform_key: str,
    project_dir: Path = PROJECT_DIR,
) -> list[Path]:
    bundle_dir = platform_bundle_dir(platform_key, project_dir)
    if not bundle_dir.exists():
        return []
    return sorted(
        path
        for path in bundle_dir.glob(artifact_glob(platform_key))
        if is_current_artifact(path.name, product_name, version, platform_key)
    )


def remaining_required_platforms(state: dict[str, Any]) -> list[str]:
    return [
        platform_key
        for platform_key in state["requiredPlatforms"]
        if state["platforms"][platform_key]["status"] != "passed"
    ]


def record_platform_passed(
    version: str,
    platform_key: str,
    artifacts: list[Path],
    version_file: Path = VERSION_FILE,
    project_dir: Path = PROJECT_DIR,
    archive_dir: Path | None = None,
    architecture: str | None = None,
) -> dict[str, Any]:
    state = load_version_state(version_file)
    if state["currentVersion"] != version:
        raise VersionStateError(
            f"待记录版本 {version} 与 version.json 中的 {state['currentVersion']} 不一致"
        )
    if state["published"]:
        raise VersionStateError(f"版本 {version} 已经发布")
    if platform_key not in SUPPORTED_PLATFORMS:
        raise VersionStateError(f"不支持的平台：{platform_key}")
    if not artifacts:
        raise VersionStateError(f"{PLATFORM_LABELS[platform_key]} 没有可记录的安装包")

    target_archive_dir = archive_dir or platform_archive_dir(platform_key, project_dir)
    archived_artifacts = archive_artifacts(artifacts, target_archive_dir, allow_replace=True)
    artifact_records = []
    for artifact in archived_artifacts:
        try:
            relative_path = artifact.relative_to(project_dir).as_posix()
        except ValueError:
            relative_path = str(artifact)
        artifact_records.append({"path": relative_path, "size": artifact.stat().st_size})

    state["platforms"][platform_key] = {
        "status": "passed",
        "architecture": architecture or normalized_architecture(),
        "testedAt": datetime.now().astimezone().isoformat(timespec="seconds"),
        "artifacts": artifact_records,
    }

    remaining = remaining_required_platforms(state)
    if not remaining:
        if any(release.get("version") == version for release in state["releases"]):
            raise VersionStateError(f"版本 {version} 已存在发布记录")
        state["published"] = True
        state["releases"].append(
            {
                "version": version,
                "published": True,
                "publishedAt": datetime.now().astimezone().isoformat(timespec="seconds"),
                "platforms": {
                    required_platform: copy.deepcopy(state["platforms"][required_platform])
                    for required_platform in state["requiredPlatforms"]
                },
            }
        )

    save_version_state(state, version_file)
    return state


def pause_on_error() -> None:
    if os.environ.get("AGENTS_LAUNCHER_COMMAND_WRAPPER") == "1":
        return
    input("\n按回车键退出……")


def main() -> int:
    try:
        platform_key = detect_current_platform()
    except VersionStateError as error:
        print(error)
        pause_on_error()
        return 1

    platform_label = PLATFORM_LABELS[platform_key]
    print(f"正在检查 {platform_label} 打包环境……")
    if not check_npm() or not check_rust():
        pause_on_error()
        return 1
    if not install_deps():
        pause_on_error()
        return 1

    try:
        version, state = prepare_build_version()
        begin_platform_build(state, platform_key)
        product_name = load_product_name()
    except (OSError, VersionStateError) as error:
        print(f"版本准备失败：{error}")
        pause_on_error()
        return 1

    if not run_build(version, product_name, platform_key):
        pause_on_error()
        return 1

    artifacts = find_built_artifacts(version, product_name, platform_key)
    if not artifacts:
        print(f"打包命令已结束，但没有找到 {platform_label} 版本 {version} 的安装包。")
        pause_on_error()
        return 1

    print("\n打包完成：")
    for artifact in artifacts:
        print(f"  {artifact}")

    test_input = input(
        f"\n{platform_label} {version} 测试通过请输入 r 后回车；"
        "测试未通过请直接回车："
    ).strip().lower()
    if test_input != "r":
        print(
            f"{platform_label} {version} 保持待测试；"
            "下一次打包仍使用该版本号。"
        )
        return 0

    try:
        updated_state = record_platform_passed(version, platform_key, artifacts)
    except (OSError, VersionStateError) as error:
        print(f"平台测试记录写入失败：{error}")
        pause_on_error()
        return 1

    remaining = remaining_required_platforms(updated_state)
    if remaining:
        labels = "、".join(PLATFORM_LABELS[item] for item in remaining)
        print(f"已记录 {platform_label} {version} 测试通过；仍需完成：{labels}。")
        print("所有必需平台通过前，版本号不会递增。")
        return 0

    print(f"版本 {version} 的所有必需平台均已通过，已记录为发布。")
    print(f"下一次任一平台打包将自动使用 {next_version(version)}。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
