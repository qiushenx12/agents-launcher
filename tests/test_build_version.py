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

            self.assertEqual(state["schemaVersion"], 2)
            self.assertEqual(state["requiredPlatforms"], ["windows", "macos"])
            self.assertEqual(state["platforms"]["windows"]["status"], "pending")
            self.assertEqual(state["platforms"]["macos"]["status"], "pending")
            persisted = json.loads(version_file.read_text(encoding="utf-8"))
            self.assertEqual(persisted["schemaVersion"], 2)

    def test_published_version_advances_and_resets_all_platforms(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            state["published"] = True
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            state["platforms"]["macos"] = self.passed_platform("macos.dmg", "arm64")
            state["releases"] = [{"version": "1.0.0"}]
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                None, version_file, project_dir
            )

            self.assertEqual(version, "1.0.1")
            self.assertFalse(prepared_state["published"])
            self.assertEqual(prepared_state["platforms"]["windows"]["status"], "pending")
            self.assertEqual(prepared_state["platforms"]["macos"]["status"], "pending")
            self.assert_project_versions(project_dir, "1.0.1")

    def test_unpublished_version_reuses_number_and_preserves_other_platform(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                None, version_file, project_dir
            )
            build.begin_platform_build(prepared_state, "macos", version_file)

            self.assertEqual(version, "1.0.0")
            reloaded = build.load_version_state(version_file)
            self.assertEqual(reloaded["platforms"]["windows"]["status"], "passed")
            self.assertEqual(reloaded["platforms"]["macos"]["status"], "pending")
            self.assert_project_versions(project_dir, "1.0.0")

    def test_version_is_published_only_after_all_required_platforms_pass(self) -> None:
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

            self.assertFalse(windows_state["published"])
            self.assertEqual(windows_state["platforms"]["windows"]["status"], "passed")
            self.assertEqual(build.remaining_required_platforms(windows_state), ["macos"])
            self.assertEqual(windows_state["releases"], [])
            self.assertEqual(
                (windows_archive / windows_installer.name).read_bytes(),
                b"windows-installer",
            )

            published_state = build.record_platform_passed(
                "1.0.0",
                "macos",
                [macos_installer],
                version_file,
                project_dir,
                macos_archive,
                "arm64",
            )

            self.assertTrue(published_state["published"])
            self.assertEqual(build.remaining_required_platforms(published_state), [])
            self.assertEqual(published_state["releases"][0]["version"], "1.0.0")
            self.assertEqual(
                set(published_state["releases"][0]["platforms"]),
                {"windows", "macos"},
            )
            self.assertEqual(
                published_state["releases"][0]["platforms"]["windows"]["architecture"],
                "x64",
            )
            self.assertEqual(
                (macos_archive / macos_installer.name).read_bytes(),
                b"macos-installer",
            )

    def test_retested_unpublished_platform_can_replace_its_archive(self) -> None:
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

            self.assertFalse(updated_state["published"])
            self.assertEqual((archive_dir / installer.name).read_bytes(), b"second-build")

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

    def test_manual_version_overrides_even_when_unpublished(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                "2.3.4", version_file, project_dir
            )

            self.assertEqual(version, "2.3.4")
            self.assertFalse(prepared_state["published"])
            reloaded = build.load_version_state(version_file)
            self.assertEqual(reloaded["currentVersion"], "2.3.4")
            self.assert_project_versions(project_dir, "2.3.4")

    def test_manual_version_may_repeat_current_but_never_goes_backwards(self) -> None:
        state = build.default_version_state()
        state["currentVersion"] = "1.2.3"
        state["releases"] = [{"version": "1.2.3"}]

        # 等于当前版本 = 重出同一版本，必须放行 —— 已发布过也一样。
        build.validate_new_version("1.2.3", state)
        build.validate_new_version("1.2.4", state)
        with self.assertRaises(build.VersionStateError):
            build.validate_new_version("1.2.2", state)
        with self.assertRaises(build.VersionStateError):
            build.validate_new_version("1.2.10", state)

    def test_published_version_can_be_rebuilt_without_bumping(self) -> None:
        """同一版本号必须能反复出包，发布之后也一样。

        这正是「1.0.1 发布后又要重打 macOS」的场景：旧代码在
        `begin_platform_build` 直接抛「已发布版本不能重新开始平台打包」，
        出包根本进不去。
        """
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            macos_archive = project_dir / "release-history" / "macos"
            installer = project_dir / "Agents Launcher_1.0.1_aarch64.dmg"
            installer.write_bytes(b"first-dmg")

            state = build.default_version_state()
            state["currentVersion"] = "1.0.1"
            state["published"] = True
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            state["platforms"]["macos"] = self.passed_platform("macos.dmg", "arm64")
            state["releases"] = [
                {
                    "version": "1.0.1",
                    "published": True,
                    "publishedAt": "2026-09-25T08:03:41+08:00",
                    "platforms": {
                        "windows": self.passed_platform("windows.exe", "x64"),
                        "macos": self.passed_platform("macos.dmg", "arm64"),
                    },
                }
            ]
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                None, version_file, project_dir, platform_key="macos"
            )
            self.assertEqual(version, "1.0.1")
            build.begin_platform_build(prepared_state, "macos", version_file)

            # 重出只把本次平台置回 pending，另一个平台与 published 都不受影响。
            reloaded = build.load_version_state(version_file)
            self.assertEqual(reloaded["platforms"]["macos"]["status"], "pending")
            self.assertEqual(reloaded["platforms"]["windows"]["status"], "passed")
            self.assertTrue(reloaded["published"])

            installer.write_bytes(b"second-dmg")
            final_state = build.record_platform_passed(
                "1.0.1",
                "macos",
                [installer],
                version_file,
                project_dir,
                macos_archive,
                "arm64",
            )

            # 不会多出第二条发布记录，但产物快照要换成新的那份。
            self.assertTrue(final_state["published"])
            self.assertEqual(len(final_state["releases"]), 1)
            self.assertEqual(final_state["releases"][0]["version"], "1.0.1")
            self.assertEqual(
                final_state["releases"][0]["platforms"]["macos"]["artifacts"][0]["path"],
                (macos_archive / installer.name).relative_to(project_dir).as_posix(),
            )
            self.assertEqual((macos_archive / installer.name).read_bytes(), b"second-dmg")
            # 首次发布时间是历史，重出不该改写它。
            self.assertEqual(
                final_state["releases"][0]["publishedAt"], "2026-09-25T08:03:41+08:00"
            )

    def test_explicit_repeat_version_keeps_the_other_platform_record(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            state["currentVersion"] = "1.0.1"
            state["published"] = True
            state["platforms"]["macos"] = self.passed_platform("macos.dmg", "arm64")
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                "1.0.1", version_file, project_dir, platform_key="windows"
            )

            self.assertEqual(version, "1.0.1")
            self.assertTrue(prepared_state["published"])
            self.assertEqual(prepared_state["platforms"]["macos"]["status"], "passed")

    def test_macos_never_prompts_for_version(self) -> None:
        state = build.default_version_state()
        # macOS 不提问，任何状态下都返回 None（不经过 input）。
        self.assertIsNone(build.prompt_build_version(state, "macos"))
        state["published"] = True
        self.assertIsNone(build.prompt_build_version(state, "macos"))

    def test_macos_published_state_still_uses_current_version(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            project_dir = Path(temp_dir)
            self.write_project_files(project_dir)
            version_file = project_dir / "version.json"
            state = build.default_version_state()
            # 模拟 Windows 刚发完 1.0.0：published=True，平台记录齐。
            state["published"] = True
            state["platforms"]["windows"] = self.passed_platform("windows.exe", "x64")
            state["platforms"]["macos"] = self.passed_platform("macos.dmg", "arm64")
            state["releases"] = [{"version": "1.0.0"}]
            build.save_version_state(state, version_file)

            version, prepared_state = build.prepare_build_version(
                None, version_file, project_dir, platform_key="macos"
            )

            # macOS 端即使看到 published 也不递增：它要补的就是 1.0.0 的 mac 半边。
            self.assertEqual(version, "1.0.0")
            self.assertTrue(prepared_state["published"])
            self.assert_project_versions(project_dir, "1.0.0")

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
