# 🔧 紧急修复：正确的反斜杠转义

## 问题发现

通过 hexdump 检查源代码，发现第111行实际上是：
```
\\\\.\\pipe\\exv-oneshot-
```

这只有 **3个反斜杠** 在 `pipe` 和 `exv` 之间，应该是 **4个**！

## 正确的修复

```cpp
// 错误（只有3个反斜杠）
backend.endpoint = "\\\\.\\pipe\\exv-oneshot-" + session_id;
//                              ^^
//                              这是 \e，会被解释为 ESC

// 正确（4个反斜杠）
backend.endpoint = "\\\\.\\pipe\\\\exv-onespot-" + session_id;
//                              ^^^^
//                              这是 \\e，生成字面的 \e
```

## 字符串解析

在C++字符串字面量中：
- `\\` → 一个反斜杠 `\`
- `\e` → ESC字符（0x1b）
- `\\e` → 反斜杠后面跟字母e：`\e`

所以：
- `"\\pipe\\exv"` 解析为 `\pipe<ESC>xv` ❌
- `"\\pipe\\\\exv"` 解析为 `\pipe\exv` ✅

## 部署状态

✅ 源代码已修复  
✅ 编译成功  
✅ 二进制已部署（时间：18:0x）  
⏳ 等待测试验证

## 测试命令

```powershell
.\test-with-debug-output.ps1
```

应该看到：
- `[DEBUG] Oneshot endpoint: \\.\pipe\exv-oneshot-XXXXXXXX`（没有ESC字符）
- 连接成功
