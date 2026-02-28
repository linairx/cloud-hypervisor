# lg-capture Cloud Hypervisor 集成

本文档描述了 lg-capture 功能在 Cloud Hypervisor 中的集成实现。

## 概述

lg-capture 集成提供以下功能：
- 帧捕获（3 缓冲写入共享内存）
- 光标数据捕获
- 音频流捕获
- 键盘/鼠标输入注入
- 多 VM 支持

## HTTP API 端点

### 输入注入

#### PUT /api/v1/vm.inject-input

注入键盘和鼠标事件。

**请求体：**
```json
{
  "backend": "ps2",
  "keyboard": [
    {"action": "press", "code": 30, "modifiers": {}},
    {"action": "release", "code": 30, "modifiers": {}}
  ],
  "mouse": [
    {"action": "move", "x": 100, "y": 50, "z": 0, "button": null, "buttons": {}},
    {"action": "button_press", "x": 0, "y": 0, "z": 0, "button": "left", "buttons": {}}
  ]
}
```

**响应：**
```json
{
  "keyboard_events": 2,
  "mouse_events": 2,
  "total_events": 4,
  "errors": 0
}
```

**支持的键盘操作：**
- `press` - 按键按下
- `release` - 按键释放
- `type` - 按下后释放

**支持的鼠标操作：**
- `move` - 相对移动
- `move_absolute` - 绝对定位
- `button_press` - 按钮按下
- `button_release` - 按钮释放
- `click` - 点击（按下+释放）
- `scroll` - 滚轮

### 帧捕获

#### GET /api/v1/vm.frame-info

获取帧缓冲区信息。

**响应：**
```json
{
  "width": 1920,
  "height": 1080,
  "format": "BGRA32",
  "buffer_count": 3,
  "frame_number": 12345,
  "active_index": 0
}
```

#### PUT /api/v1/vm.frame-capture.start

开始帧捕获。

#### PUT /api/v1/vm.frame-capture.stop

停止帧捕获。

#### GET /api/v1/vm.frame-capture.status

获取捕获状态。

**响应：**
```json
{
  "capturing": true,
  "format": "BGRA32",
  "width": 1920,
  "height": 1080,
  "buffer_count": 3,
  "frame_count": 12345,
  "active_index": 1,
  "guest_state": "Capturing"
}
```

#### PUT /api/v1/vm.frame-capture.set-format

设置帧格式。

**请求体：**
```json
{
  "format": "BGRA32",
  "width": 1920,
  "height": 1080
}
```

### 光标信息

#### GET /api/v1/vm.cursor-info

获取光标信息。

**响应：**
```json
{
  "x": 100,
  "y": 200,
  "visible": true,
  "width": 32,
  "height": 32,
  "hot_x": 0,
  "hot_y": 0,
  "has_shape": true,
  "update_count": 42
}
```

## 配置选项

### IVSHMEM 帧缓冲区配置

```bash
--ivshmem "path=/dev/shm/fb,size=64M,frame_buffer=1920x1080:BGRA32:3"
```

参数说明：
- `path`: 共享内存文件路径
- `size`: 共享内存大小（必须足够容纳帧数据）
- `frame_buffer`: 帧缓冲区配置，格式为 `WIDTHxHEIGHT:FORMAT:BUFFER_COUNT`
  - `WIDTH`: 帧宽度（像素）
  - `HEIGHT`: 帧高度（像素）
  - `FORMAT`: 像素格式（BGRA32, RGBA32, NV12）
  - `BUFFER_COUNT`: 缓冲区数量（默认3）

## 共享内存布局

```
+------------------+  <- 偏移 0
| FrameBufferHeader|  (96 bytes)
+------------------+
| FrameMetadata[0] |  (40 bytes each)
| FrameMetadata[1] |
| FrameMetadata[2] |
+------------------+
| Buffer[0] data   |  (width × height × bpp)
| Buffer[1] data   |
| Buffer[2] data   |
+------------------+
| CursorMetadata   |  (32 bytes)
+------------------+
| CursorShapeInfo   |  (32 bytes)
+------------------+
| Cursor data      |  (max 64KB)
+------------------+
| AudioBufferHeader|  (96 bytes)
+------------------+
| Audio ring buffer|  (1MB default)
+------------------+
```

## 后端选择

### PS/2 (推荐用于自动化)
- 最高隐蔽性
- 模拟真实硬件
- 所有操作系统原生支持

### VirtIO Input
- 现代化
- 支持绝对坐标
- 但易被检测为虚拟设备

### USB HID (计划中)
- 需要 xHCI 实现
- 平衡的隐蔽性

## Guest Agent 协议

### 命令/状态协议

Host 通过 `command` 字段发送命令：
- `None` - 空闲
- `StartCapture` - 开始捕获
- `StopCapture` - 停止捕获
- `SetFormat` - 设置格式

Guest Agent 通过 `guest_state` 字段报告状态：
- `Idle` - 空闲
- `Capturing` - 正在捕获
- `Error` - 错误状态
- `Initializing` - 初始化中

### 帧写入流程

1. Guest Agent 检测 `command == StartCapture`
2. 获取下一个缓冲区索引
3. 写入帧数据
4. 更新 `FrameMetadata`
5. 更新 `active_index`
6. 递增 `frame_count`

### 帧读取流程

1. Host 读取 `active_index`
2. 读取对应 `FrameMetadata`
3. 读取帧数据
4. 处理帧

## 使用示例

### 注入键盘输入

```bash
# 按下 'A' 键
curl -X PUT http://localhost/api/v1/vm.inject-input \
  -H "Content-Type: application/json" \
  -d '{"keyboard":[{"action":"press","code":30}]}'

# 释放 'A' 键
curl -X PUT http://localhost/api/v1/vm.inject-input \
  -H "Content-Type: application/json" \
  -d '{"keyboard":[{"action":"release","code":30}]}'
```

### 注入鼠标移动

```bash
# 移动鼠标
curl -X PUT http://localhost/api/v1/vm.inject-input \
  -H "Content-Type: application/json" \
  -d '{"mouse":[{"action":"move","x":100,"y":50}]}'

# 点击
curl -X PUT http://localhost/api/v1/vm.inject-input \
  -H "Content-Type: application/json" \
  -d '{"mouse":[{"action":"button_press","button":"left"},{"action":"button_release","button":"left"}]}'
```

### 启动帧捕获

```bash
# 开始捕获
curl -X PUT http://localhost/api/v1/vm.frame-capture.start

# 查看状态
curl -X GET http://localhost/api/v1/vm.frame-capture.status

# 停止捕获
curl -X PUT http://localhost/api/v1/vm.frame-capture.stop
```

## 键盘码参考

常用键盘码（PC Scancode Set 1）：

| 键 | Code |
|---|------|
| A | 0x1E |
| B | 0x30 |
| C | 0x2E |
| D | 0x20 |
| Enter | 0x1C |
| Space | 0x39 |
| Escape | 0x01 |
| Left Ctrl | 0x1D |
| Left Alt | 0x38 |
| Left Shift | 0x2A |

## 实现状态

| 功能 | 状态 |
|------|------|
| PS/2 键盘注入 | ✅ 完成 |
| PS/2 鼠标注入 | ✅ 完成 |
| VirtIO Input 框架 | ✅ 完成 |
| 帧缓冲区数据结构 | ✅ 完成 |
| IVSHMEM 集成 | ✅ 完成 |
| HTTP API | ✅ 完成 |
| 光标数据支持 | ✅ 完成 |
| 音频数据结构 | ✅ 完成 |
| Guest Agent 协议 | ✅ 完成 |
| USB HID 后端 | ⏳ 计划中 |
