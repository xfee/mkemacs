# mkemacs

秒开Emacs 通过键盘钩子实现Emacs 风格的快捷键，在整个Windows系统全局可用

## 原理

将 CapsLock 通过注册表映射为 F9（相当于 Emacs 的 Ctrl/C-x 万能键），然后用本程序的底层键盘钩子拦截 F9 + 其他键的组合，翻译成 Emacs 风格的操作。

## 快捷键

| 组合 | 等价操作 |
|------|---------|
| `CapsLock + A` | Home（行首） |
| `CapsLock + E` | End（行尾） |
| `CapsLock + B` | Left（左移一个字符） |
| `CapsLock + F` | Right（右移一个字符） |
| `CapsLock + P` | Up（上移一行） |
| `CapsLock + N` | Down（下移一行） |
| `CapsLock + D` | Delete（删除光标后字符） |
| `CapsLock + H` | Backspace（删除光标前字符） |
| `CapsLock + K` | 删除至行尾（Shift+End, Delete） |

## 使用

1. 用 [SharpKeys](https://sharpkeys.net/) 将 CapsLock 映射为 F9（修改注册表，需注销重新登录）
2. 下载 [mkemacs.exe](https://github.com/xfee/mkemacs/releases) 并运行
3. 托盘图标右键可禁用/启用、查看使用说明

## 构建

```bash
cargo build --release
# 输出: target/release/mkemacs.exe
```

## 技术

- Windows `WH_KEYBOARD_LL` 低级键盘钩子
- 独立线程 `SendInput` 防递归
- 无 GC、无运行时依赖，单文件 Rust
