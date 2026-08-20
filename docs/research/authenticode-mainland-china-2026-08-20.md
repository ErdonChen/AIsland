# AIsland Authenticode 路径核验（中国大陆个人，2026-08-20）

## 结论

截至 2026-08-20，AIsland 作为 Windows-only、Apache-2.0 开源项目，在“中国大陆个人开发者 + 直接从 GitHub 分发 NSIS `.exe` + 云签/CI”这一约束下，实际路径按优先级如下：

1. **SignPath Foundation：可以立即申请，适合作为零采购成本的首选尝试；是否获批尚未确认。** 官方为符合条件的开源项目免费提供由 SignPath Foundation 持有的证书、HSM 托管和 GitHub Actions 集成，不要求项目维护者做个人身份验证。但项目要经过人工审核，证书中的发布者是 `SignPath Foundation`，每个发布签名请求都要求人工批准；官方还明确要求项目具备可验证的声誉，没有公布最低 star 或发布时间门槛。因此这是一条真实可申请路径，不是保证获批的路径。
2. **SSL.com IV Code Signing + eSigner：目前证据最完整的付费、个人、云签、无人值守 CI 路径。** SSL.com 官方销售 Individual Validated Code Signing，官方接受列表包含中国 `CN`，eSigner 明确支持 IV 证书、Windows Authenticode、GitHub Actions、SignTool/CKA 和无交互 headless signing。最终能否签发仍取决于用户完成并通过证件、活体/人工材料和地址验证。
3. **Certum Open Source Code Signing + SimplySign：产品能力成立，但今天不能视为可落地方案。** 官方产品页当前显示缺货；官方没有给出“中国大陆自然人可获签发”的明确资格清单；官方操作流程依赖手机生成 token 登录 SimplySign Desktop，必要时还会弹出 PIN，未找到官方的无人值守 GitHub Actions/API 代码签名方案。购买前必须取得 Certum 对中国大陆个人签发和 CI 自动化的书面确认。
4. **Microsoft Azure Artifact Signing Public Trust：不可用。** 当前官方资格仍限定个人开发者位于美国或加拿大；中国大陆个人不符合 Public Trust 资格。Private Trust 不受此地理限制，但不能为普通消费者提供默认受 Windows 信任的发布者链，因此不能替代 Public Trust Authenticode。
5. **Microsoft Store MSIX：可用，但属于另一条分发路线。** 中国在 Store 开发者账户支持地区列表中，个人账户免费；通过 Store 提交 MSIX 后由 Microsoft 重签，可避免 Store 安装场景的 SmartScreen 下载警告。它不会给 GitHub 上现有的 NSIS 安装包签名，也不能替代直接下载渠道的 Authenticode。

## 状态定义

- **confirmed**：官方产品/资格/技术文档共同覆盖本项目所需能力；最终签发仍可能取决于正常身份或项目审核。
- **not confirmed**：产品存在，但官方材料没有确认关键的国家、申请人类型或无人值守 CI 条件。
- **unavailable**：官方资格或当前产品状态明确阻止使用。

## 路径对比

| 路径 | 中国大陆个人资格 | Authenticode / 时间戳 | 云签 / CI | 当前状态 | 对 AIsland 的判断 |
| --- | --- | --- | --- | --- | --- |
| SignPath Foundation OSS | 不做个人身份验证；条款未把维护者国籍/居住地列为资格条件，但项目须审核 | 托管证书支持 Authenticode；file-based signing 自动管理时间戳 | 官方 GitHub Action、REST API、HSM；每个发布须人工批准 | **confirmed service / acceptance not confirmed** | 立即申请；低成本，但发布者显示 SignPath Foundation，且新项目声誉能否过审未知 |
| SSL.com IV + eSigner | 官方接受国家代码包含 China/CN；IV 面向个人 | `.exe/.dll/.msi` 等 Authenticode；官方 RFC 3161 时间戳 `http://ts.ssl.com` | eSigner CKA/API；官方明确 GitHub Actions 和 headless signing | **confirmed applicant path** | 最清晰的付费 CI 方案；签发需用户完成身份与地址验证 |
| Certum Open Source + SimplySign | 仅限个人，材料清楚；中国大陆签发资格无官方明确结论 | 微软信任、`.exe/.msi`、时间戳；云端 HSM/虚拟卡 | 云端私钥成立，但官方流程要求手机 token / 可能 PIN；无人值守 CI 未确认 | **not confirmed / product out of stock** | 不应直接下单，也不应先围绕它锁定工作流 |
| Azure Artifact Signing Public Trust | 个人仅限美国、加拿大 | Public Trust Authenticode；托管时间戳 | 原生 CI/CD | **unavailable** | 中国大陆个人不能使用；Private Trust 不解决公共分发信任 |
| Microsoft Store MSIX | 中国在账户支持地区；个人账户免费 | Store 审核后由 Microsoft 重签 | Store 发布可自动化，但不是给 GitHub EXE 云签 | **confirmed alternative** | 可以并行做 MSIX/Store；不覆盖现有 NSIS 直链下载 |

## 1. Microsoft Azure Artifact Signing

### Public Trust 资格：unavailable

Microsoft 的当前 Quickstart 明确写明：Public Trust 可供若干地区的组织使用，但**个人开发者必须位于美国或加拿大**；该地理限制只是不适用于 Private Trust。中国大陆个人因此仍不能申请 Artifact Signing Public Trust。[Microsoft：Set up Artifact Signing，Prerequisites](https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart?tabs=registerrp-portal%2Caccount-portal%2Ccertificateprofile-portal)

Microsoft 2026 年的 Windows 代码签名选项页也重复了同一限制，并说明 Artifact Signing 没有硬件 token、可以直接接入 CI/CD，但新签名仍需积累 SmartScreen 声誉。[Microsoft：Code signing options for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)

结论：历史资格没有发生能帮助本用户的变化。不要创建 Azure 资源、付费或把 Private Trust 当成消费者可信 Authenticode 的替代品。

## 2. Certum Open Source Code Signing + SimplySign

### 产品和签名能力：confirmed

Certum 官方产品页说明 Open Source Code Signing：

- 面向公开的免费/开源软件；
- 证书受 Microsoft 信任，可用于积累 SmartScreen 声誉；
- SimplySign 把私钥放在云端，不需要物理卡和读卡器；
- 支持 `.exe`、`.msi`、`.dll` 等格式，并提供时间戳；
- 证书主体为自然人，名称带 `Open Source Developer`；
- 每月最多 5,000 次签名。

但同一官方页面在核验当天显示 **Product is out of stock**。[Certum：Open Source Code Signing in the Cloud](https://shop.certum.eu/open-source-code-signing-on-simplysign.html)

### 个人条件与材料：confirmed

Certum 官方要求该产品只签发给个人；需验证订阅人身份，并提交：

- 自动身份验证、注册点/身份确认点、公证确认，或手持有效身份证件照片中的一种验证方式；
- 有效身份证、护照、驾照或永久居留卡等材料；
- 订阅人名下的水、电、燃气、电话等账单；
- 正在维护的公开开源项目网址，页面必须能清楚证明申请人与项目的关系；
- 项目若用于分发商业软件，证书可能被吊销。

[Certum：Code Signing — required documents](https://support.certum.eu/en/code-signing-required-documents/)

AIsland 的 Apache-2.0 仓库、公开 Releases 和维护者身份能满足“公开开源项目及关系可见”这一表面材料要求，但是否最终满足 Certum 审核只能由 Certum 决定。

### 中国大陆个人资格：not confirmed

本次没有在 Certum 的官方产品页、签发材料页或支持文档中找到按国家列出的签发资格，也没有找到明确写着“接受中国大陆自然人申请 Open Source Code Signing”的说明。Certum 曾发布与中国合作伙伴推广 Code Signing 的官方新闻，但这只能证明其在中国开展渠道合作，不能证明波兰直营商店会向中国大陆个人签发该具体产品。[Certum：中国合作伙伴新闻](https://www.certum.eu/en/news/certum-conquers-the-chinese-cybersecurity-market/)

因此中国大陆资格必须保持为 **not confirmed**。应在付款前让 Certum 支持书面确认以下三点：

1. 接受中国大陆居民以个人身份申请 Open Source Code Signing；
2. 接受中国护照或居民身份证以及中国地址账单；
3. 证书链被当前 Windows Trusted Root Program 信任，可用于 Authenticode `.exe`/NSIS。

### 云签与 CI：cloud confirmed / unattended CI not confirmed

SimplySign 确实是云端虚拟加密卡。官方要求安装手机端 SimplySign 和桌面端 SimplySign Desktop，后者把云端证书模拟为电脑上的加密卡。[Certum：SimplySign applications](https://support.certum.eu/en/installation-of-the-simplysign-applications/)

然而官方 SignTool 指南要求先用手机生成 token 登录 SimplySign Desktop；对有 PIN 的虚拟卡，第一次签名还会弹出 PIN 输入框。官方材料只证明交互式 SignTool/Jarsigner 使用，没有证明 GitHub-hosted runner 可以无交互、无人值守地完成签名。[Certum：Code Signing in the Cloud — SignTool/Jarsigner PDF](https://files.certum.eu/documents/manual_en/CS-Code_Signing_in_the_Cloud_Signtool_jarsigner_signing.pdf)

结论：SimplySign 是云签，但不能据此推导出它是 CI-friendly 的 signing API。除非 Certum 提供针对该 Open Source 产品的官方 unattended/CI 方案，否则不要把它接入 AIsland 的稳定发布工作流。

## 3. SSL.com IV Code Signing + eSigner

### 中国大陆个人申请路径：confirmed applicant path

SSL.com 的官方 eSigner 产品页直接销售 **IV (Individual Validated) Code Signing**，并说明 IV、OV、EV 三类 SSL.com 代码签名证书都可接入 eSigner。[SSL.com：eSigner Cloud Code Signing Service](https://www.ssl.com/products/software-integrity/signing-service/)

SSL.com 的官方“可接受 CSR 国家代码”页明确把 **China / CN** 列在接受列表中；该页说明这是证书注册人的原属国家代码列表。[SSL.com：Accepted Country Codes for CSRs](https://secure.ssl.com/csrs/country_codes.html)

结合 IV 产品和中国 `CN` 接受列表，可以确认中国大陆个人有一条官方文档支持的申请路径。这里的 **confirmed** 不等于保证签发：用户仍需通过 SSL.com 的个人验证。

### 身份材料：confirmed

SSL.com 的 2026 年个人身份验证指南说明 IV Code Signing 需要 IV，优先流程使用手机拍摄证件并完成活体检测；如果自动验证在所在地区不可用或证件识别失败，可改为人工上传。人工流程需要身份证件正反面/护照资料页和一张申请人手持证件的照片；证件必须能证明姓名、出生年份、照片，并可能要求补充地址材料或邮寄验证地址。[SSL.com：Identity Validation for SSL.com Certificates](https://www.ssl.com/guide/identity-validation-for-ssl-com-certificates-a-complete-guide/)

SSL.com 的代码签名验证文档也明确区分 Individual Validation 与组织/EV 验证，并列出政府签发照片证件、证件背面和手持证件照片。[SSL.com：Validation Process for Code Signing Certificates](https://www.ssl.com/how-to/validation-process-for-document-signing-code-signing-and-ev-code-signing-certificates/)

### Authenticode、云签和 CI：confirmed

eSigner 官方说明：

- 支持 `.exe`、`.dll`、`.msi`、`.cab`、`.sys`、`.ps1` 等 Authenticode 格式；
- 私钥保存在 SSL.com 的 FIPS 140-2 Level 3 云 HSM；
- eSigner CKA 可直接与 `signtool.exe` 集成；
- 支持 GitHub Actions、GitLab CI、Jenkins、Azure DevOps 等；
- 支持完全无人值守的 headless signing；
- IV、OV、EV 证书均可使用；
- 证书和 eSigner 是两个分别购买/订阅的产品。

[SSL.com：eSigner Cloud Code Signing Service](https://www.ssl.com/products/software-integrity/signing-service/)

SSL.com 官方时间戳命令使用 RFC 3161 端点：

```powershell
signtool sign /fd sha256 /tr http://ts.ssl.com /td sha256 /a .\AIsland.exe
```

`/tr` 必须搭配 `/td sha256`；时间戳使签名在代码签名证书到期后仍可根据签名时点验证。[SSL.com：Getting Started With Your Code Signing Certificate](https://www.ssl.com/how-to/getting-started-with-your-code-signing-certificate-installation-configuration-and-your-first-signing-operation/)

### 成本快照

核验日的官方页面显示 IV Code Signing 为 **USD 129/年**，eSigner 最低层为 **USD 15/月**、每月 240 次签名。价格会变化，付款前应重新查看官方结账页；本研究没有下单。[SSL.com：eSigner pricing](https://www.ssl.com/products/software-integrity/signing-service/)

### 仍需用户本人完成

1. 付款前向 SSL.com 支持确认中国大陆居民的 IV Code Signing + eSigner 订单和中国证件/地址材料组合；虽然官网证据支持申请，本步骤能避免支付后才发现个案材料问题。
2. 用户本人购买、提交证件、活体或手持证件照片、完成地址验证。
3. 证书签发并开通 eSigner 后，把 eSigner 的 CI 凭据作为 GitHub Actions secrets 配置；不得提交到仓库。

## 4. SignPath Foundation 开源签名

### 服务、HSM 和自动化：confirmed

SignPath Foundation 官方说明，它为开源项目免费提供代码签名证书：无需维护者个人身份验证，Foundation 验证二进制确实来自开源仓库，并使用自己的名义担保；私钥在 SignPath HSM 中生成和保存，支持自动化构建接入。[SignPath Foundation：首页](https://signpath.org/)

官方 GitHub 集成通过 `signpath/github-action-submit-signing-request@v2` 上传 GitHub Actions artifact、提交签名请求、等待并下载签名产物；对 OSS 项目，签名前的 job 必须运行在 GitHub-hosted runner。[SignPath：GitHub trusted build system](https://docs.signpath.io/trusted-build-systems/github)

SignPath 的 artifact configuration 原生支持 Authenticode `.exe`、`.dll`、`.msi`、`.msix` 等；file-based signing 会自动管理时间戳。[SignPath：Artifact Configuration / Authenticode](https://docs.signpath.io/artifact-configuration/reference)、[SignPath：Timestamps](https://docs.signpath.io/crypto-providers/)

### AIsland 资格与限制：acceptance not confirmed

SignPath 的官方条款要求：

- 使用 OSI 批准的开源许可证，不能对仓库内组件做商业双重许可；
- 不包含维护者或关联方发布的专有组件；
- 项目已发布且持续维护；
- 所有团队成员对 GitHub 和 SignPath 开启 MFA；
- 定义作者/提交者、审阅者、签名批准者角色；
- 每个发布签名请求都必须由批准者人工批准；
- README/下载页必须明确写出 `Code signing policy`，列出签名服务、团队角色和隐私政策；
- Foundation 会审核项目声誉和控制权，未知项目没有当然获得证书的权利。

[SignPath Foundation：Conditions for Open Source projects](https://signpath.org/terms.html)

AIsland 已具备 Apache-2.0、公开源码、公开 Releases、隐私说明和 CI，表面上满足多项基础条件。`docs/open-core.md` 中未来 Pro 代码放在独立代码库/独立模块的安排需要在申请材料中解释清楚，避免被误解为对当前仓库做商业双重许可。由于 AIsland 目前发布时间短、社区声誉数据很少，Foundation 是否接受只能由其审核决定；官方没有 star 数门槛，不能自行声称一定通过。

另一个关键权衡是：签名的发布者显示 **SignPath Foundation**，而不是 `Erdon Chen` 或 `AIsland`。它可以消除“未知发布者”并建立有效 Authenticode 信任，但品牌身份不是项目作者本人。

申请前无需购买；需要先完成以下公开仓库准备：

1. 在 README 和发布页增加 `Code signing policy` 链接；
2. 在 `docs/code-signing.md` 列出提交者/审阅者与签名批准者；
3. 链接 `PRIVACY.md` 并确认联网行为披露完整；
4. 确认 GitHub 维护者已启用 MFA；
5. 申请获批后再安装 SignPath GitHub App、创建 API token 和 secrets。

## 5. Microsoft Store MSIX

这是并行分发方案，不是给现有 GitHub NSIS 安装包发证书。

Microsoft 官方地区列表包含中国，个人开发者账户目前免费；个人开户需要 Microsoft 账户、政府签发身份证件和自拍验证。[Microsoft：Developer account locations and fees](https://learn.microsoft.com/zh-cn/windows/apps/publish/partner-center/account-types-locations-and-fees)、[Microsoft：Open an individual developer account](https://learn.microsoft.com/en-us/windows/apps/publish/partner-center/open-a-developer-account)

对 MSIX/AppX Store 提交，开发者不需要自己的 CA 代码签名证书；应用通过认证后由 Store 用 Microsoft 证书重签。对 MSI/EXE Store 路径则不同：Microsoft 不会重签，发布者仍必须自己提供受信任的 Authenticode 签名。[Microsoft Store signing FAQ](https://learn.microsoft.com/en-us/windows/apps/publish/faq/get-started-with-the-microsoft-store)

因此：

- 若 AIsland 能产出并通过认证的 MSIX，Store 用户可以获得可信安装体验；
- GitHub Releases 中的 NSIS `.exe` 仍要走 SignPath、SSL.com 或其他 Public Trust 证书；
- Store MSIX 与 GitHub NSIS 可以并行存在，不应混为同一种签名资产。

## 6. Authenticode、时间戳与 SmartScreen 必须分开

### Authenticode 有效性

有效 Authenticode 证明二进制在签名后未被修改，并把它关联到证书中的发布者身份。AIsland 的稳定发布门禁应继续要求：

```powershell
$signature = Get-AuthenticodeSignature -LiteralPath .\AIsland_0.1.0_x64-setup.exe
$signature.Status -eq [System.Management.Automation.SignatureStatus]::Valid
$null -ne $signature.SignerCertificate
$null -ne $signature.TimeStamperCertificate
```

### RFC 3161 时间戳

时间戳证明代码是在证书有效期内签署，使签名能在发布者证书到期后继续验证。它不是 SmartScreen 声誉，也不能把无效/不受信任证书变成有效证书。

### SmartScreen 声誉

Microsoft 当前文档明确说明：OV、EV 和 Artifact Signing 的新文件/新发布者都可能先显示 SmartScreen 警告；EV 自 2024 年起不再自动绕过 SmartScreen。声誉综合发布者证书和具体文件哈希，随干净下载和使用积累。Store 分发的应用由 Microsoft 签名，不受 SmartScreen 下载警告。[Microsoft：SmartScreen reputation for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)

因此不要向用户宣传“买 EV 就立即无蓝屏”，也不要依赖 Tauri 文档中仍保留的旧版 EV 即时声誉说法；此问题应以 Microsoft 2026 年文档为准。

## 7. AIsland 仓库接入检查

### 当前已具备

- `docs/code-signing.md` 已正确区分 Tauri updater signature 与 Windows Authenticode。
- `.github/workflows/release-windows.yml` 已把稳定发布锁在 `AUTHENTICODE_RELEASE_ENABLED=true` 后，并在发布前检查 Authenticode `Valid`、签名者证书和时间戳证书。
- 未签名 Preview 工作流明确只允许 `NotSigned` 的技术预览，不冒充稳定版或 Microsoft 信任。

### 当前缺口

- `src-tauri/tauri.conf.json` 的 `bundle.windows` 目前没有 `signCommand`、`certificateThumbprint` 或 `timestampUrl`。
- 稳定发布工作流没有调用 SignPath、eSigner 或任何其他 Authenticode provider；打开布尔门禁不会自动产生签名。
- 当前 `tauri-action` 会在构建期间生成 Tauri updater `.sig`，但若后续再由外部服务修改安装包以加入 Authenticode，先前生成的 updater 签名将不再对应最终字节。接入外部 signer 时必须保证：**先得到最终 Authenticode-signed installer，再对该最终文件生成 Tauri updater signature 和 `latest.json`**；或者把 Authenticode provider 通过 Tauri `bundle.windows.signCommand` 接入构建内部，让 updater signature 在其后生成。[Tauri：Windows Code Signing / custom sign command](https://v2.tauri.app/distribute/sign/windows/)、[Tauri：Updater signatures](https://v2.tauri.app/plugin/updater/)

### Provider 未选定前可以安全完成的准备

1. 保持 `AUTHENTICODE_RELEASE_ENABLED` 默认关闭。
2. 把稳定发布流程拆为明确阶段：build unsigned → provider sign → `Get-AuthenticodeSignature` 验证 → Tauri updater sign → draft asset validation → publish。
3. 为 provider 集成预留命名清晰的 GitHub environment/secrets，但不要创建空 secrets、示例私钥或把凭据写进仓库。
4. 加入对 signer subject、时间戳存在性、最终 installer SHA-256 和 updater signature 的自动验证。
5. 若先申请 SignPath，先补齐其要求的公开 `Code signing policy`；若选 SSL.com，等用户完成购买/身份验证并取得官方 eSigner GitHub Actions 参数后，再锁定具体 action/安装器版本和 secret 名称。

## 推荐决策

### 现在做

1. **立即提交 SignPath Foundation 申请**，但不要承诺通过；同时完成其 Code signing policy、角色和隐私披露要求。
2. **把 SSL.com IV + eSigner 作为付费保底方案**。付款前只做一次官方支持确认，确认中国大陆个人证件/地址材料和 GitHub-hosted runner 方案；之后由用户本人接管购买与身份验证。
3. **暂缓 Certum 购买**，直到商品恢复库存，并取得中国大陆个人签发 + 无人值守 CI 的书面确认。
4. **明确排除 Artifact Signing Public Trust**，避免浪费 Azure 注册和验证时间。
5. **并行评估 Store MSIX**，用于普通用户获取可信安装体验；继续为 GitHub NSIS 保留独立 Authenticode 路线。

### 唯一需要用户接管的敏感步骤

当路径确定后，用户本人负责购买/付款、身份证件与活体/自拍、地址证明、账户协议，以及在 GitHub UI 中创建 provider secrets。仓库侧的工作流、验证门禁和公开政策可以在这些步骤之前准备，但没有真实 provider 凭据时不能声称签名已经完成。
