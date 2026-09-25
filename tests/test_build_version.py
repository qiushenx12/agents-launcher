import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import build


class BuildVersionTests(unittest.TestCase):
    def test_install_deps_syncs_existing_node_modules(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            (project_dir / "package.json").write_text("{}", encoding="utf-8")
            (project_dir / "node_modules").mkdir()

            completed = build.subprocess.CompletedProcess(["npm", "install"], 0)
            with (
                patch.object(build, "PROJECT_DIR", project_dir),
                patch.object(build, "find_npm", return_value="npm"),
                patch.object(build.subprocess, "run", return_value=completed) as run,
            ):
                self.assertTrue(build.install_deps())

            run.assert_called_once_with(
                ["npm", "install", "--no-audit", "--no-fund"], cwd=project_dir
            )

    def test_next_version_increments_patch_until_nine(self) -> None:
        self.assertEqual(build.next_version("1.0.0"), "1.0.1")
        self.assertEqual(build.next_version("1.0.8"), "1.0.9")
        self.assertEqual(build.next_version("1.0.9"), "1.1.0")
        self.assertEqual(build.next_version("1.9.9"), "1.10.0")

    def test_patch_greater_than_nine_is_rejected(self) -> None:
        with self.assertRaises(build.VersionStateError):
            build.next_version("1.0.10")

    def test_schema_one_state_is_migrated_to_platform_records(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            version_file = Path(temp_dir) / "version.json"
            version_file.write_text(
                json.dumps(
                    {
                        "schemaVersion": 1,
                        "currentVersion": "1.0.0",
                        "published": False,
                        "releases": [],
                    }
                ),
                encoding="utf-8",
            )

            state = build.load_version_state(version_file)

            self.assertEqual(state["schemaVersion"], 3)
            self.assertEqual(state["platforms"]["windows"]["status"], "pending")
            self.assertEqual(state["platforms"]["macos"]["status"], "pending")
            persisted = json.loads(version_file.read_text(encoding="utf-8"))
            self.assertEqual(persisted["schemaVersion"], 3)

    def test_migration_drops_the_publish_controls(self) -> None:
        """schema 2 的 `published` / `requiredPlatforms` 必须被丢弃。

        它们让「直接回车」的含义取决于一个看不见的标志，正是本次要取消的多重
        控制。`releases` 保留为历史，`publishedAt` 改名为中性的 `recordedAt`。
        """
        with tempfile.TemporaryDirectory() as temp_dir:
            version_file = Path(temp_dir) / "version.json"
            version_file.write_text(
                json.dumps(
                    {
                        "schemaVersion": 2,
                        "currentVersion": "1.0.1",
                        "published": True,
                        "requiredPlatforms": ["windows", "macos"],
                        "platforms": {
                            "windows": self.passed_platform("windows.exe", "x64"),
                            "macos": self.passed_platform("macos.dmg", "arm64"),
                        },
                        "releases": [
                            {
                                "version": "1.0.1",
                                "published": True,
                                "publishedAt": "2026-09-25T08:03:41+08:00",
                                "platforms": {
                                    "windows": self.passed_platform("windows.exe", "x64"),
                                    "macos": self.passed_platform("macos.dmg", "arm64"),
                                },
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )

            state = build.load_version_state(version_file)

            self.assertEqual(state["schemaVersion"], 3)
            self.assertNotIn("published", state)
            self.assertNotIn("requiredPlatforms", state)
            # 平台状态与历史都保住了。
            self.assertEqual(state["platforms"]["windows"]["status"], "passed")
            self.assertEqual(len(state["releases"]), 1)
            self.assertNotIn("published", state["releases"][0])
            self.assertEqual(
                state["releases"][0]["recordedAt"], "2026-09-25T08:03:41+08:00"
            )
            self.assertIn("platforms", state["releases"][0])

    def test_enter_always_keeps_the_current_version(self) -> None:
        """直接回车 = 沿用当前版本，永远如此。

        这里曾经有第二条路径：`published` 为真时自动升一版。于是同一个按键在
        不同时刻做不同的事，取决于用户看不见的标志 —— 已移除。
        """
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            # 两个平台都 passed 的状态，正是旧代码里会自动升版的那种。
            state = build.default_version_state()
            state["currentVersion"] = "1.0.1"
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            state["platforms"]["macos"] = self.passed_platform("macos.dmg", "arm64")
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(None, version_file, project_dir)

            self.assertEqual(version, "1.0.1")
            # 平台记录也不该被重置 —— 本次只是重出，不是开新版本。
            self.assertEqual(prepared_state["platforms"]["windows"]["status"], "passed")
            self.assertEqual(prepared_state["platforms"]["macos"]["status"], "passed")
            self.assert_project_versions(project_dir, "1.0.1")

    def test_higher_version_resets_both_platform_records(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            state["currentVersion"] = "1.0.1"
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                "1.0.2", version_file, project_dir
            )

            self.assertEqual(version, "1.0.2")
            self.assertEqual(prepared_state["platforms"]["windows"]["status"], "pending")
            self.assertEqual(prepared_state["platforms"]["macos"]["status"], "pending")
            self.assert_project_versions(project_dir, "1.0.2")

    def test_same_version_keeps_the_other_platform_record(self) -> None:
        """输入与当前相同的版本号 = 重出，不动另一个平台的记录。

        否则重出 mac 会顺手把 windows 刚做好的 `passed` 抹掉。
        """
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            state["currentVersion"] = "1.0.1"
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                "1.0.1", version_file, project_dir
            )
            build.begin_platform_build(prepared_state, "macos", version_file)

            self.assertEqual(version, "1.0.1")
            reloaded = build.load_version_state(version_file)
            self.assertEqual(reloaded["platforms"]["windows"]["status"], "passed")
            self.assertEqual(reloaded["platforms"]["macos"]["status"], "pending")
            self.assert_project_versions(project_dir, "1.0.1")

    def test_manual_version_may_repeat_current_but_never_goes_backwards(self) -> None:
        state = build.default_version_state()
        state["currentVersion"] = "1.2.3"
        state["releases"] = [{"version": "1.2.3"}]

        # 等于当前版本 = 重出同一版本，必须放行。
        build.validate_new_version("1.2.3", state)
        build.validate_new_version("1.2.4", state)
        with self.assertRaises(build.VersionStateError):
            build.validate_new_version("1.2.2", state)
        with self.assertRaises(build.VersionStateError):
            build.validate_new_version("1.2.10", state)

    def test_packaging_records_both_platforms_without_a_gate(self) -> None:
        """平台各记各的，互不为前提。没有「都通过才算发布」这道门禁。"""
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            version_file = project_dir / "version.json"
            windows_archive = project_dir / "release-history" / "windows"
            macos_archive = project_dir / "release-history" / "macos"
            windows_installer = project_dir / "Agents Launcher_1.0.0_x64-setup.exe"
            macos_installer = project_dir / "Agents Launcher_1.0.0_aarch64.dmg"
            windows_installer.write_bytes(b"windows-installer")
            macos_installer.write_bytes(b"macos-installer")
            build.save_version_state(build.default_version_state(), version_file)

            windows_state = build.record_platform_passed(
                "1.0.0",
                "windows",
                [windows_installer],
                version_file,
                project_dir,
                windows_archive,
                "x64",
            )

            self.assertEqual(windows_state["platforms"]["windows"]["status"], "passed")
            # macOS 还没打过包，但这不妨碍 Windows 的产物被记录与归档。
            self.assertEqual(windows_state["platforms"]["macos"]["status"], "pending")
            self.assertEqual(
                (windows_archive / windows_installer.name).read_bytes(),
                b"windows-installer",
            )
            self.assertEqual(windows_state["releases"][0]["version"], "1.0.0")
            self.assertEqual(set(windows_state["releases"][0]["platforms"]), {"windows"})

            final_state = build.record_platform_passed(
                "1.0.0",
                "macos",
                [macos_installer],
                version_file,
                project_dir,
                macos_archive,
                "arm64",
            )

            self.assertEqual(final_state["platforms"]["macos"]["status"], "passed")
            self.assertEqual(
                (macos_archive / macos_installer.name).read_bytes(),
                b"macos-installer",
            )
            # 同一版本只有一条历史，两个平台的快照都在里面。
            self.assertEqual(len(final_state["releases"]), 1)
            self.assertEqual(
                set(final_state["releases"][0]["platforms"]), {"windows", "macos"}
            )

    def test_retested_platform_can_replace_its_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            version_file = project_dir / "version.json"
            archive_dir = project_dir / "release-history" / "windows"
            installer = project_dir / "Agents Launcher_1.0.0_x64-setup.exe"
            build.save_version_state(build.default_version_state(), version_file)
            installer.write_bytes(b"first-build")
            build.record_platform_passed(
                "1.0.0",
                "windows",
                [installer],
                version_file,
                project_dir,
                archive_dir,
                "x64",
            )

            state = build.load_version_state(version_file)
            build.begin_platform_build(state, "windows", version_file)
            installer.write_bytes(b"second-build")
            updated_state = build.record_platform_passed(
                "1.0.0",
                "windows",
                [installer],
                version_file,
                project_dir,
                archive_dir,
                "x64",
            )

            self.assertEqual((archive_dir / installer.name).read_bytes(), b"second-build")
            # 重出刷新那条历史，而不是追加第二条。
            self.assertEqual(len(updated_state["releases"]), 1)
            self.assertEqual(updated_state["releases"][0]["version"], "1.0.0")

    def test_rebuild_keeps_the_first_recorded_time(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            version_file = project_dir / "version.json"
            archive_dir = project_dir / "release-history" / "macos"
            installer = project_dir / "Agents Launcher_1.0.0_aarch64.dmg"
            build.save_version_state(build.default_version_state(), version_file)
            installer.write_bytes(b"first-dmg")

            first = build.record_platform_passed(
                "1.0.0", "macos", [installer], version_file, project_dir, archive_dir, "arm64"
            )
            first_recorded_at = first["releases"][0]["recordedAt"]

            installer.write_bytes(b"second-dmg")
            second = build.record_platform_passed(
                "1.0.0", "macos", [installer], version_file, project_dir, archive_dir, "arm64"
            )

            self.assertEqual(len(second["releases"]), 1)
            # 首次记录时间是历史，重出不该改写它。
            self.assertEqual(second["releases"][0]["recordedAt"], first_recorded_at)

    def test_platform_detection_and_build_commands(self) -> None:
        self.assertEqual(build.detect_current_platform("win32"), "windows")
        self.assertEqual(build.detect_current_platform("darwin"), "macos")
        with self.assertRaises(build.VersionStateError):
            build.detect_current_platform("linux")
        self.assertEqual(
            build.platform_build_command("npm", "windows"),
            ["npm", "run", "tauri", "build"],
        )
        self.assertEqual(
            build.platform_build_command("npm", "macos"),
            ["npm", "run", "tauri", "build", "--", "--bundles", "app,dmg"],
        )

    def test_macos_never_prompts_for_version(self) -> None:
        state = build.default_version_state()
        # macOS 不提问，任何状态下都返回 None（不经过 input）。
        self.assertIsNone(build.prompt_build_version(state, "macos"))

    @staticmethod
    def passed_platform(path: str, architecture: str) -> dict[str, object]:
        return {
            "status": "passed",
            "architecture": architecture,
            "testedAt": "2026-07-21T12:00:00+08:00",
            "artifacts": [{"path": path, "size": 1}],
        }

    @staticmethod
    def write_project_files(project_dir: Path) -> None:
        tauri_dir = project_dir / "src-tauri"
        tauri_dir.mkdir(parents=True)
        (project_dir / "package.json").write_text(
            json.dumps({"name": "agents-launcher", "version": "0.0.0"}),
            encoding="utf-8",
        )
        (project_dir / "package-lock.json").write_text(
            json.dumps(
                {
                    "name": "agents-launcher",
                    "version": "0.0.0",
                    "packages": {"": {"name": "agents-launcher", "version": "0.0.0"}},
                }
            ),
            encoding="utf-8",
        )
        (tauri_dir / "tauri.conf.json").write_text(
            json.dumps({"productName": "Agents Launcher", "version": "0.0.0"}),
            encoding="utf-8",
        )
        (tauri_dir / "Cargo.toml").write_text(
            '[package]\nname = "agents-launcher"\nversion = "0.0.0"\n\n[dependencies]\n',
            encoding="utf-8",
        )
        (tauri_dir / "Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "agents-launcher"\nversion = "0.0.0"\n',
            encoding="utf-8",
        )

    def assert_project_versions(self, project_dir: Path, version: str) -> None:
        package_data = json.loads((project_dir / "package.json").read_text(encoding="utf-8"))
        package_lock = json.loads((project_dir / "package-lock.json").read_text(encoding="utf-8"))
        tauri_config = json.loads(
            (project_dir / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8")
        )
        cargo_toml = (project_dir / "src-tauri" / "Cargo.toml").read_text(encoding="utf-8")
        cargo_lock = (project_dir / "src-tauri" / "Cargo.lock").read_text(encoding="utf-8")
        self.assertEqual(package_data["version"], version)
        self.assertEqual(package_lock["version"], version)
        self.assertEqual(package_lock["packages"][""]["version"], version)
        self.assertEqual(tauri_config["version"], version)
        self.assertIn(f'version = "{version}"', cargo_toml)
        self.assertIn(f'version = "{version}"', cargo_lock)


if __name__ == "__main__":
    unittest.main()
