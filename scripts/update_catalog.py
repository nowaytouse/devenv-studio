#!/usr/bin/env python3
"""Import the pinned WinUtil WinGet application map and curated developer tasks.

The source is downloaded separately from the fixed upstream commit. This tool
refuses any bytes other than the reviewed snapshot and preserves the existing
catalog task IDs and revision.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_SHA256 = "5072ff3f3b4c689db313000c8ef7ff93c655f5cea55c5990192ead87789ecf41"
SOURCE_URL = "https://github.com/ChrisTitusTech/winutil/blob/7514faf0d0f4bff9b6c5d9213426ab52520cfac4/config/applications.json"
WINGET_HELP = "https://learn.microsoft.com/windows/package-manager/winget/install"
REVISION = "2026-10-23.1"
NPM_IDS = {"typescript", "typescript-language-server", "prettier", "npm-check-updates", "http-server", "serve"}
TARGETS = ["win10_ltsc2021", "win11_ltsc2024"]
PACKAGE_LIMITATIONS = (
    "需要用户已配置的 App Installer / WinGet 和 winget 来源，可使用网络。安装版本由来源当前可用版本决定，"
    "可能请求管理员权限。不会自动接受来源或软件许可；如需确认许可，请先在官方 WinGet 中手动完成。"
    "无法通过注册表恢复卸载软件。"
)
DEV_LIMITATIONS = {
    "cargo": "使用当前用户可用的 Rust/Cargo 工具链和 crates.io 网络源；按 Crate 名执行 cargo install，不固定版本。"
    "请先检查软件许可及 Windows 兼容性。Cargo 构建可能执行 crate 提供的构建脚本；本任务不修改系统 PATH。",
    "uv": "使用已安装的 uv 与当前用户的 Python 环境，并访问 PyPI；不固定包版本。"
    "请先检查软件许可和包的适用性。包安装或构建后端可能执行发布者提供的代码；本任务不修改系统 PATH。",
    "vscode": "使用已安装的 Visual Studio Code 和 Marketplace 网络源；扩展版本由执行时来源决定。"
    "请先检查发布者、权限、许可与兼容性；扩展可能在 VS Code 中运行代码。不会自动启动扩展或修改其权限。",
}
CATEGORY_ZH = {
    "Browsers": "浏览器",
    "Communications": "通信",
    "Development": "开发",
    "Document": "文档",
    "Games": "游戏",
    "Microsoft Tools": "微软工具",
    "Multimedia Tools": "多媒体",
    "Pro Tools": "专业工具",
    "Selfhosted Tools": "自托管工具",
    "Utilities": "实用工具",
}
CATEGORY_EN = {
    "Browsers": "browsers",
    "Communications": "communications",
    "Development": "development tools",
    "Document": "document tools",
    "Games": "games",
    "Microsoft Tools": "Microsoft tools",
    "Multimedia Tools": "multimedia tools",
    "Pro Tools": "professional tools",
    "Selfhosted Tools": "self-hosted tools",
    "Utilities": "utilities",
}
CARGO = [
    ("ripgrep", "ripgrep", "递归文本搜索工具，适合在代码与文档中按模式查找内容。"),
    ("fd-find", "fd-find", "用于按名称和路径查找文件的命令行工具。"),
    ("bat", "bat", "带语法高亮与分页显示的命令行文件查看器。"),
    ("eza", "eza", "用于列出目录内容的命令行工具。"),
    ("zoxide", "zoxide", "根据使用记录快速跳转目录的命令行工具。"),
    ("starship", "starship", "可配置的跨 Shell 命令提示符。"),
    ("just", "just", "用于定义和运行项目常用命令的任务运行器。"),
    ("git-delta", "git-delta", "用于改善 Git diff 输出可读性的分页与差异查看工具。"),
]
UV = [
    ("ruff", "ruff", "Python 代码检查与格式化工具。"),
    ("yamllint", "yamllint", "用于检查 YAML 文件格式与常见问题的工具。"),
    ("httpie", "httpie", "用于发送和检查 HTTP 请求的命令行客户端。"),
    ("pre-commit", "pre-commit", "在提交前按项目配置运行代码检查钩子的框架。"),
]
VSCODE = [
    ("rust-lang.rust-analyzer", "rust-lang.rust-analyzer", "为 Visual Studio Code 提供 Rust 语言分析、补全和导航功能。"),
    ("tamasfe.even-better-toml", "tamasfe.even-better-toml", "为 Visual Studio Code 提供 TOML 语法支持。"),
    ("ms-python.python", "ms-python.python", "为 Visual Studio Code 提供 Python 语言支持与调试集成。"),
    ("charliermarsh.ruff", "charliermarsh.ruff", "将 Ruff 检查与格式化集成到 Visual Studio Code。"),
    ("esbenp.prettier-vscode", "esbenp.prettier-vscode", "将 Prettier 格式化器集成到 Visual Studio Code。"),
    ("dbaeumer.vscode-eslint", "dbaeumer.vscode-eslint", "将 ESLint 检查集成到 Visual Studio Code。"),
    ("ms-vscode.powershell", "ms-vscode.powershell", "为 Visual Studio Code 提供 PowerShell 编辑与调试支持。"),
    ("ms-azuretools.vscode-docker", "ms-azuretools.vscode-docker", "为 Visual Studio Code 提供 Docker 开发集成。"),
]


def slug(value: str) -> str:
    value = re.sub(r"[^a-z0-9]+", "-", value.lower()).strip("-")
    if not value or len(value) > 72:
        raise ValueError(f"Unsafe task id segment: {value!r}")
    return value


def line_for_key(source_text: str, key: str) -> int:
    quoted = json.dumps(key, ensure_ascii=False)
    pattern = re.compile(r"^\s*" + re.escape(quoted) + r"\s*:\s*\{")
    for number, line in enumerate(source_text.splitlines(), 1):
        if pattern.match(line):
            return number
    raise ValueError(f"Cannot locate WinUtil source entry {key!r}")


def developer_task(provider: str, package: str, description_zh: str, evidence: str) -> tuple[dict, dict]:
    task_id = f"developer-{provider}-{slug(package)}"
    provider_title = {"cargo": "Cargo crate", "uv": "uv tool", "vscode": "VS Code extension"}[provider]
    title = f"{package} ({provider_title})"
    description_en = {
        "cargo": "Install the named Rust crate as a command-line tool with Cargo.",
        "uv": "Install the named Python command-line tool with uv.",
        "vscode": "Install the named extension in the current user's Visual Studio Code profile.",
    }[provider]
    task = {
        "id": task_id,
        "title": title,
        "description": description_zh,
        "category": "developer",
        "kind": provider,
        "targets": TARGETS,
        "requires_admin": False,
        "restart": "通常无需重启；新命令或扩展可能需要重新打开终端或编辑器。",
        "limitations": DEV_LIMITATIONS[provider],
        "evidence": [evidence],
        "package_id": package,
        "feature_name": None,
    }
    english = {
        "title": title,
        "description": description_en,
        "category": "Developer tools",
        "restart": "Usually no restart is needed; a new command or extension may require reopening the terminal or editor.",
        "limitations": {
            "cargo": "Uses the current user's Rust/Cargo toolchain and crates.io network source; installs the current version without a version pin. Review the crate's license and Windows compatibility first. Cargo builds may run crate-provided build scripts. The task does not change system PATH.",
            "uv": "Uses an installed uv and the current user's Python environment with access to PyPI; installs the current version without a version pin. Review the package license and suitability first. Package installation or build backends may run publisher-provided code. The task does not change system PATH.",
            "vscode": "Uses an installed Visual Studio Code and its Marketplace network source; the extension version is whatever the source offers at execution time. Review publisher, permissions, license, and compatibility first. Extensions can run code inside VS Code; their permissions are not changed automatically.",
        }[provider],
    }
    return task, english


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path, help="Local bytes downloaded from the pinned raw GitHub URL")
    parser.add_argument("--check", action="store_true", help="Validate the already-imported catalog without writing files")
    args = parser.parse_args()
    raw = args.source.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != SOURCE_SHA256:
        raise SystemExit(f"Refusing unreviewed WinUtil source: SHA-256 {digest}")
    source_text = raw.decode("utf-8")
    upstream = json.loads(source_text)
    catalog_path = ROOT / "data/deployment.json"
    locale_path = ROOT / "data/locales/en-US.json"
    catalog = json.loads(catalog_path.read_text(encoding="utf-8"))
    locale = json.loads(locale_path.read_text(encoding="utf-8"))
    if args.check:
        source_ids = []
        for item in upstream.values():
            package_id = item.get("winget")
            if not isinstance(package_id, str) or package_id.lower().startswith("msstore:"):
                continue
            if len(package_id) > 100 or "." not in package_id or not all(
                ch.isascii() and (ch.isalnum() or ch in "-_.+") for ch in package_id
            ):
                raise SystemExit(f"Invalid pinned WinGet ID: {package_id!r}")
            source_ids.append(package_id.casefold())
        tasks = catalog["tasks"]
        task_ids = [task["id"] for task in tasks]
        package_ids = [task["package_id"].casefold() for task in tasks if task["kind"] == "package"]
        preserved = [task for task in tasks if not task["id"].startswith(("software-winutil-", "developer-"))]
        developer = [task for task in tasks if task["id"].startswith("developer-")]
        expected_dev = {
            developer_task(provider, package, description, "https://placeholder.invalid")[0]["id"]
            for provider, entries in [("cargo", CARGO), ("uv", UV), ("vscode", VSCODE)]
            for package, _, description in entries
        } | {f"developer-npm-{slug(package)}" for package in NPM_IDS}
        if (
            catalog.get("revision") != REVISION
            or len(tasks) != 271
            or len(set(task_ids)) != len(task_ids)
            or len(source_ids) != 234
            or len(set(source_ids)) != 234
            or len(package_ids) != 234
            or set(package_ids) != set(source_ids)
            or len(preserved) != 41
            or len(developer) != 26
            or {task["id"] for task in developer} != expected_dev
            or set(locale["tasks"]) != set(task_ids)
        ):
            raise SystemExit("Imported catalog IDs, counts, revision, or English overlay do not match the pinned source.")
        print("Catalog check passed: 234 pinned WinGet IDs, 26 developer tasks, 41 preserved tasks, 271 English task entries; no files changed.")
        return
    if catalog.get("revision") != REVISION:
        raise SystemExit("Catalog revision changed; review and update this importer explicitly.")
    if len(catalog["tasks"]) != 41 or len(locale["tasks"]) != 41:
        raise SystemExit("Expected the preserved 41-task catalog and English overlay.")
    original_ids = {task["id"] for task in catalog["tasks"]}
    if len(original_ids) != 41:
        raise SystemExit("Existing deployment catalog has duplicate task IDs.")
    original_packages = {task["package_id"] for task in catalog["tasks"] if task.get("package_id")}
    if len(original_packages) != 30:
        raise SystemExit("Expected 30 existing WinGet package IDs.")
    packages = {value.lower() for value in original_packages}
    task_ids = set(original_ids)
    imported = []
    ignored_store = []
    seen_upstream = set()
    for key, item in upstream.items():
        package_id = item.get("winget")
        if not isinstance(package_id, str):
            continue
        if package_id.lower().startswith("msstore:"):
            ignored_store.append(package_id)
            continue
        if len(package_id) > 100 or "." not in package_id or not all(
            ch.isascii() and (ch.isalnum() or ch in "-_.+") for ch in package_id
        ):
            raise SystemExit(f"Unreviewed WinGet identifier shape: {package_id!r}")
        folded = package_id.casefold()
        if folded in seen_upstream:
            raise SystemExit(f"Duplicate WinGet identifier in pinned source: {package_id}")
        seen_upstream.add(folded)
        if folded in {value.casefold() for value in packages}:
            continue
        upstream_url = SOURCE_URL + f"#L{line_for_key(source_text, key)}"
        title = item.get("content")
        category = item.get("category")
        if not isinstance(title, str) or not title.strip() or category not in CATEGORY_ZH:
            raise SystemExit(f"Missing reviewed name/category for {key}")
        task_id = "software-winutil-" + slug(key)
        if task_id in task_ids:
            raise SystemExit(f"Task ID collision: {task_id}")
        task_ids.add(task_id)
        packages.add(folded)
        category_zh = CATEGORY_ZH[category]
        category_en = CATEGORY_EN[category]
        task = {
            "id": task_id,
            "title": title,
            "description": f"WinUtil 固定清单将此软件归类为「{category_zh}」；通过 WinGet 安装 {title}。请先查阅发布者说明并确认许可、配置和适用性。",
            "category": "software",
            "kind": "package",
            "targets": TARGETS,
            "requires_admin": False,
            "restart": "以安装器结果为准；不会自动重启",
            "limitations": PACKAGE_LIMITATIONS,
            "evidence": [upstream_url, WINGET_HELP],
            "package_id": package_id,
            "feature_name": None,
        }
        imported.append(task)
        locale["tasks"][task_id] = {
            "title": title,
            "description": f"WinUtil lists this under {category_en}. Installs {title} through WinGet; review the publisher's terms, setup, and suitability before proceeding.",
            "category": "Software",
            "restart": "Follow the installer result; no automatic restart.",
            "limitations": "Requires the user-configured App Installer/WinGet and configured WinGet sources; network access may be needed. The installed version is whichever version the source currently offers and may request administrator approval. The app does not automatically accept source or software agreements. If an agreement must be accepted, review and accept it manually in official WinGet first. Registry restoration cannot uninstall software.",
        }
    if len(seen_upstream) != 234 or len(ignored_store) != 2:
        raise SystemExit(f"Unexpected source mapping: {len(seen_upstream)} package IDs, {len(ignored_store)} Store entries.")
    if packages != {task["package_id"].casefold() for task in catalog["tasks"] if task.get("package_id")} | {
        task["package_id"].casefold() for task in imported
    }:
        raise SystemExit("Package identity accounting failed.")
    curated = []
    for provider, entries, host in [
        ("cargo", CARGO, "https://crates.io/crates/"),
        ("uv", UV, "https://pypi.org/project/"),
        ("vscode", VSCODE, "https://marketplace.visualstudio.com/items?itemName="),
    ]:
        for package, _, description in entries:
            evidence = host + package
            task, translation = developer_task(provider, package, description, evidence)
            if task["id"] in task_ids or package.casefold() in packages:
                raise SystemExit(f"Developer task collision: {task['id']}")
            task_ids.add(task["id"])
            curated.append(task)
            locale["tasks"][task["id"]] = translation
    if len(curated) != 20:
        raise SystemExit("Expected 20 curated developer tasks.")
    catalog["tasks"].extend(imported)
    catalog["tasks"].extend(curated)
    if len(catalog["tasks"]) > 2048 or len(locale["tasks"]) != len(catalog["tasks"]):
        raise SystemExit("Catalog or locale bounds/coverage failed.")
    for path, value in [(catalog_path, catalog), (locale_path, locale)]:
        path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(
        f"Imported {len(imported)} WinGet packages; retained {len(original_packages)} existing packages; "
        f"added {len(curated)} developer tasks; total={len(catalog['tasks'])}; "
        f"WinGet source IDs={len(seen_upstream)}, Store IDs intentionally excluded={len(ignored_store)}."
    )


if __name__ == "__main__":
    main()
