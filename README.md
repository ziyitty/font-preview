# 本机字体预览

原生 Windows 桌面程序（Rust + egui + DirectWrite/GDI），无浏览器、无后台服务。
读取系统字体库，输入文字后逐字检查字形覆盖，只保留能完整渲染这段文字的字体，
以卡片列表实时预览；支持字体名模糊搜索与拼音搜索。

## 运行

双击 `字体预览.exe`（单文件，无依赖）。

自己编译：

```bash
cargo build --release
copy target\release\font_preview.exe 字体预览.exe
```

## 用法

- 上方输入框：想要预览的文字，支持多行；卡片按该字体实时渲染。
- 搜索框：字体名、拼音全拼、拼音首字母都能搜；
  `weiruan` / `wryh` → 微软雅黑，`fangsong` → 仿宋，`msyh` → Microsoft YaHei（子序列模糊匹配）。
- 字号滑杆 20–72 px，卡片高度随之变化。
- 点击卡片复制字体族名；Ctrl+F 聚焦搜索框。
- 计数显示「匹配数 / 本机字体总数」。

## 结构

```
src/main.rs    // eframe 窗口、顶部栏、卡片网格（虚拟滚动）、纹理缓存
src/fonts.rs   // DirectWrite 枚举字族 + HasCharacter 逐字覆盖 + 拼音/模糊打分
src/gdi.rs     // GDI 把文字画进位图，量行高、垂直居中，转成 egui 纹理
```

## 实现要点

- 字体枚举：`IDWriteFactory::GetSystemFontCollection`，按 family 归并，取 en-us 名给 GDI，zh-cn 名做展示。
- 覆盖检测：`IDWriteFont::HasCharacter(codepoint)` 逐字判定，空白、控制符、变体选择符、ZWJ 不参与；
  结果按 `(字体序号, 字符)` 缓存，重复输入直接命中。
- 搜索：每个字族预生成归一化匹配串（英文名、中文名、全拼、首字母），
  打分顺序为前缀 > 子串 > 子序列，含中文的名字用 `pinyin` crate 转拼音。
- 渲染：不用把字体文件读进内存，GDI 直接用系统字体绘制到 DIB，
  扫描墨迹范围做垂直居中后上传为纹理；只栅格化可见卡片，纹理按 (字体, 文字, 尺寸) 缓存并做 LRU 淘汰。
- 界面字体：读 `msyh.ttc` 注册进 egui，保证中文标签正常显示。

## 平台

Windows（依赖 DirectWrite + GDI）。换 macOS/Linux 需要替换 `fonts.rs` 的枚举和 `gdi.rs` 的栅格化。
