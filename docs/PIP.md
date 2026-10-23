# Pip：解释器、用户范围与验收边界

0.8的Pip适配器属于开发任务，复用同一份v3工作区、逐项审核、取消、有界输出和执行历史。37个内置发行包包含旧工具固定清单全部26项。导入只匹配可信的裸发行包名，不执行导入文件中的命令、版本条件、URL或额外参数。

## 发现与运行预检

GUI启动/刷新只读PythonCore注册和安装文件，显示解释器候选及预期用户目录，不自动运行Python。支持标准、稳定、64位CPython3.11–3.14；按已支持版本确定候选，优先处理相同注册标识的HKCU配置。实际执行仍需运行预检，候选存在不等于pip可用。

不从PATH、WindowsApps别名、Conda、pyenv、Scoop shim或嵌入式分发猜测Python。注册路径必须为绝对本地标准python.exe，符合安装根目录且无额外ExecutableArguments；拒绝不安全的文件/目录与嵌入式._pth。注册信息不等于发布者签名认证。

预检核对实际解释器路径、CPython稳定版本、位数、安装根、非虚拟环境、user-site启用、与当前用户RoamingAppData对应的userbase/site/scripts，以及可用的标准安装pip。已激活VIRTUAL_ENV/CONDA_PREFIX、非空PYTHONHOME/PYTHONPATH/PYTHONUSERBASE/PYTHONNOUSERSITE等环境不被静默改写，而是明确拒绝。实际解释器和目录随检查/执行记录保留。

## 安装与回查

- 每项任务固定同一解释器和布局，进行预检、包列表检查、必要时安装、重新预检及包身份回查。
- Python使用-E -P，保留user-site的正常可见性。不能使用-I或-s搭配新版pip的显式--user安装，否则user-site禁用会被pip拒绝。
- Pip使用--isolated、--disable-pip-version-check与--no-input；移除子进程PIP配置覆盖，设置精确的小写PIP_CONFIG_FILE=nul，避免加载global/site/user配置；不修改原配置文件。
- 安装固定--user、--only-binary=:all:及https://pypi.org/simple，不添加--upgrade、--force-reinstall、--ignore-installed、--break-system-packages或bootstrap。没有兼容wheel的项目/依赖明确失败，不fallback构建源码。
- 包列表核对该解释器可见的全局与用户发行包；pip允许全局包满足用户安装请求，因此不能只查--user。请求的editable安装不自动视为受信任已安装包。
- 裸发行包名按PEP503规范化大小写及连续的点、下划线、短横线。JSON必须完整，重复字段/规范化身份、异常版本、输出错误及元数据警告不能被当作未安装或安装成功。
- 安装退出0但回查缺失、解释器身份/作用域变化、取消、超时或输出不完整均不能算完成。

用户依赖可能随解析安装而变化；没有通用卸载/回滚。只确认发行包元数据可见，不保证Python导入、CLI、Jupyter服务器或本地运行库已验收，也不保证与Mac版版本/构建一致。wheel-only限制构建流程，不是Python运行时沙箱；保留user-site意味着已有的.pth/usercustomize等正常Python启动逻辑可能运行。当前不修改PATH，可从显示的解释器或Scripts目录使用工具；项目应自行管理虚拟环境及锁定依赖。

## 组合与迁移

新增python-data组合明确选择已有可信目录的Python.Python.3.14、Git、VS Code、6个Pip项目及Python扩展。组合仅提议选择；用户审核加入后，软件前置任务先执行，Pip再解析可用解释器。原有python工具组合保持其uv工作流。

旧SetupProfile packages.pip_packages中的精确受信任包可单独审核映射。既有Mac schema1没有Pip包数组，不虚构扩展字段；七个固定旧版formula映射可指向对应Pip发行包，保留功能差异和来源。Pip镜像、任意需求条件、私密配置及未审核包继续手动，不隐含改写pip.ini。Ruff的既有formula→uv映射不改变。

## Primary references

- [Python命令行-E/-P与用户site](https://docs.python.org/3.14/using/cmdline.html)
- [PEP514：Windows注册发现](https://peps.python.org/pep-0514/)
- [Pip用户安装规则](https://pip.pypa.io/en/stable/user_guide/#user-installs)
- [Pip install](https://pip.pypa.io/en/stable/cli/pip_install/)
- [Pip list JSON](https://pip.pypa.io/en/stable/cli/pip_list/)
- [Pip配置来源与空设备禁用](https://pip.pypa.io/en/stable/topics/configuration/)
- [Pip26.2.2配置加载源码](https://github.com/pypa/pip/blob/26.2.2/src/pip/_internal/configuration.py)
- [发行包名称规范](https://packaging.python.org/en/latest/specifications/name-normalization/)
- [旧版固定Pip清单](https://github.com/nowaytouse/LTSC_Tools_Backup/blob/a8523abbe08147b4d5de8746f26d99d056057ab0/src/assets/setup_profile.json)
