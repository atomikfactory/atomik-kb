# Welcome to **atomik-kb**

Rich text with *italic*, <u>underline</u>, ~~strike~~, `code` and a [link](https://example.com/docs).  
See also [Linked page](Linked%20page.md) and [Linked page](Linked%20page.md).

- First item
  - Nested item
- Second item due <time datetime="2026-10-07" data-reminder="1440">2026-10-07</time>, review <time datetime="2026-10-08T14:30" data-reminder="15">2026-10-08 14:30</time>

3. Third step

- [x] Done
- [ ] To do

> A quoted line with $E=mc^2$ inline.

```rust
fn main() {
    println!("hi");
}
```

$$
E = mc^2
$$

---

<details open>
<summary><h2>Toggle summary</h2></summary>

Hidden until opened.

</details>

<!-- atomik:{"attrs":{"color":"blue","icon":"💡"},"node":"callout"} -->

> 💡 Callouts carry an icon.

| Feature | Status | Notes |
| --- | --- | --- |
| Tables | **Done** | Header row<br><br>Plain cells |
| Pipes | a \| b | Escaped in Markdown |

<!-- atomik:{"attrs":{"align":"right","width":640,"widthPercent":60}} -->

![Diagram of the sync flow](Welcome/diagram.png "Figure 1: the \"sync\" flow, C:\\sync")

<!-- atomik:{"attrs":{"view":"preview"}} -->

[Quarterly report.pdf](Welcome/Quarterly%20report.pdf)

<!-- atomik:{"attrs":{"columns":3},"items":3,"node":"gallery"} -->

![](Welcome/diagram.png "Opening night, \"north\" wing")

![](Welcome/photo%20(2).jpg)

![](attachment://8c3f4a5b-6d7e-4f80-91a2-b3c4d5e6f708 "Backstage")

<!-- atomik:{"attrs":{"height":360},"file":"Welcome/Whiteboard.excalidraw","node":"whiteboard"} -->

> **Whiteboard**
>
> - Plan A ship it

<!-- atomik:{"node":"columnList"} -->

<!-- atomik:{"node":"column"} -->

Left column

<!-- atomik:{"node":"column"} -->

Right column

Two blocks in this one

<!-- atomik:{"end":"columnList"} -->

[Linked page](Linked%20page.md)

Unicode: café, 日本語, 👩‍💻 and 𝔘𝔫𝔦𝔠𝔬𝔡𝔢.

<!-- atomik:{"attrs":{"label":"Add task","position":"above"},"node":"templateButton"} -->

Task for @Ada due <time datetime="2026-10-18" data-reminder="30" data-relative="today">2026-10-18</time>

<!-- atomik:{"end":"templateButton"} -->

<!-- atomik:{"node":"tableOfContents"} -->

<!-- atomik:{"node":"breadcrumb"} -->

<!-- atomik:{"attrs":{"layout":"list","properties":["Tags","Priority"],"sort":{"by":"Priority","dir":"desc"},"source":{"kind":"tag","option":"work"}},"node":"pageList"} -->
