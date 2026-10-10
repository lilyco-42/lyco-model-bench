# lyco-model-bench

每日 **模型发现 + token 测速**：NVIDIA NIM / OpenRouter 的模型供给每天都在变
（加新、下架、改名、悄悄禁用的都有），本工具每天跑一趟，把「当前可用」的模型
和「中文回答有多快」测出来，输出带推荐的报告。

Rust 重写版：用 lilyco 一个 `struct` 出 **CLI / TUI / Web / MCP** 四端。同一份二进制
既能在 Windows 任务计划里跑，也能拷到 Radxa 上 cron 跑，还能让 QQ 机器人走 MCP
直接调。

## 构建

```bash
cargo build --release
```

产物在 `target/release/lyco-model-bench`（Windows 下是 `.exe`）。默认特性只含
CLI + MCP；桌面想用 TUI / Web GUI 再开：

```bash
cargo build --release --features tui,web
```

## 密钥

只从两个地方读，**永不打印**（任何日志都不含密钥值）：

1. 环境变量 `NIM_API_KEY` / `OPENROUTER_API_KEY`
2. `~/.config/lyco-llm/keys.env`（`export KEY=value` 一行一个）

```
OPENROUTER_API_KEY=sk-or-...
NIM_API_KEY=nvapi-...
```

keys 文件路径可用 `--keys` 覆盖（比如自建机上挂在别处）。

## 用法

```bash
# 发现：列出 provider 当前有的模型
lyco-model-bench discover

# 探测：每个模型发一个 1-token 请求，把卡死/下线的挑出来（并发 8、单次超时 8s）
lyco-model-bench probe

# 测速：发现 → 探测 → 固定中文提示实测 token 速度（reps=3 取中位数，最多 512 token）
lyco-model-bench bench
lyco-model-bench bench --provider nim
lyco-model-bench bench --model nvidia/nemotron-3-super-120b-a12b --reps 5

# 每日任务：全流程跑完，把 latest.json / latest.md / recommend.json 写进 --out 目录
lyco-model-bench daily --out results

# 报表：把落盘的 JSON 渲染成 Markdown（离线，不联网）
lyco-model-bench report
```

每条命令都带一堆可选参数，缺省即合理：`--scope free`（OpenRouter 默认 free）、
`--timeout 60`、`--probe-timeout 8`、`--concurrency 8`、`--reps 3`、
`--max-tokens 512`，以及内置的中文提示词（描写春天西湖、约 120 字）。想要给
agent 用的 JSON Schema，任意命令加 `--schema` 即可打印。

## 四端（lilyco）

同一个二进制，五种用法：

| 入口 | 说明 |
| --- | --- |
| `lyco-model-bench <cmd>` | CLI（脚本 / 定时任务用的就是这个） |
| `lyco-model-bench --tui` | TUI 命令选择页（需 `--features tui`） |
| `<cmd> --gui` | Web GUI（需 `--features web`） |
| `lyco-model-bench --mcp` | MCP stdio 服务器，Agent 直接调 discover / probe / bench / daily / report |

MCP 在默认特性里，`bench --schema` 打印的 30 秒「结构化工具定义」同样适用。

## 定时跑

- **Windows**：任务计划程序每天 05:30 跑 `lyco-model-bench daily --out ...\results`
  （本机已注册同名任务）。
- **Linux / Radxa（常开机）**：cron：

```cron
30 5 * * * radxa /home/radxa/lyco-model-bench/target/release/lyco-model-bench daily --out /home/radxa/lyco-model-bench/results >> /home/radxa/lyco-model-bench/results/cron.log 2>&1
```

## 结果

`results/` 目录（gitignored，CI 不生成）：

- `latest.md` — 人类可读的表格，末尾有一行 `RECOMMEND`（最快的能答中文的模型）
- `latest.json` — 机器可读：`rows[]` + `discovery` + `recommend`
- `recommend.json` — 单独抽出的推荐，给下游（比如 QQ 机器人）消费

指标含义：`tok/s` 是含 TTFT 的墙钟吞吐；`ttft(ms)` 指到第一个**回答** token 的时间
（reasoning 阶段不计）；reps 是取中位数的重复次数。注意 reasoning 模型
（如 `nvidia/nemotron-3-super-120b-a12b`、`gpt-oss-20b`）会把 512 token 的预算烧在
隐藏思考上，出现 `answered=no` 不是 bug——报告里单独给了 `reason_chars` 说明。

## CI

GitHub Actions **只编译 + 测试 / 检查**：fmt、clippy `-D warnings`、
`build --locked`、`test --locked`，在 ubuntu + windows 上跑。**绝不在 CI 里联网跑
benchmark**——测速需要真实 API keys，而且每天的结果本来就该落在本地/自建机。

## Python 旧版

`python/` 里有 `bench.py`（纯标准库）+ `run_daily.cmd` / `run_daily.sh`，是本工具的
Python 原型。Rust 版功能等价（还多出 MCP / TUI / Web 三端），旧脚本保留供回看，
不再演进。

## License

MIT（见 LICENSE）。