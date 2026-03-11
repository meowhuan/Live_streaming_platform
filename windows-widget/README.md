# Windows 弹幕小组件（WPF）

一个独立的 Windows WPF 小组件，用于实时显示弹幕（WS + API）。
整体 UI 采用 meow 深色风格 + 毛玻璃层次，并保持无边框圆角窗口。

## 启动

```powershell
cd windows-widget
# 首次运行请指定 DOTNET_CLI_HOME（避免权限问题）
$env:DOTNET_CLI_HOME = "f:\Live_streaming_platform\.dotnet"
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = "1"

dotnet run
```

## 发布

```powershell
cd windows-widget
$env:DOTNET_CLI_HOME = "f:\Live_streaming_platform\.dotnet"
$env:DOTNET_SKIP_FIRST_TIME_EXPERIENCE = "1"

dotnet publish -c Release
```

发布输出在：
`windows-widget\bin\Release\net10.0-windows\publish\`

## 配置

`settings.json`（会复制到输出目录）：

```json
{
  "ApiBase": "http://127.0.0.1:5174",
  "WsUrl": ""
}
```

- `ApiBase`：你的后端地址（用于拉取 `/api/chat/latest`）
- `WsUrl`：可留空，会自动根据 ApiBase 生成 `ws(s)://.../ws`

环境变量优先级更高：
- `MEOW_WIDGET_API_BASE`
- `MEOW_WIDGET_WS_URL`

## 功能

- 无边框窗口 + 系统圆角（Windows 11）
- meow 风格深色渐变面板
- 置顶开关 / 关闭按钮
- 实时弹幕更新
- 自动滚动 + 新消息提示

## 说明

- 需要 Windows 11 才能获得最佳圆角与毛玻璃效果。
- 如遇到 dotnet 构建权限问题，请按上面的 `DOTNET_CLI_HOME` 设置方式运行。

