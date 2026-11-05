import { useEffect, useRef, useState } from "react";
import { Terminal, ArrowUpRight, ArrowDown, Copy, Check, X, AppleLogo, WindowsLogo, TerminalWindow, Pause, Play, DownloadSimple } from "@phosphor-icons/react";

const repository = "https://github.com/qmtdlt/sshm";
const installCommand = "curl -fsSL https://raw.githubusercontent.com/qmtdlt/sshm/main/install.sh | sh";
const asset = (name) => `${import.meta.env.BASE_URL}assets/${name}`;

function ExternalLink({ children, href, className = "", icon: Icon = ArrowUpRight, ...props }) {
  return <a href={href} className={className} target="_blank" rel="noopener noreferrer" {...props}>{children}<Icon aria-hidden="true" /></a>;
}

function Wordmark() {
  return <span className="wordmark"><Terminal weight="bold" aria-hidden="true" /><span>sshm</span></span>;
}

function CopyButton({ text, label = "复制" }) {
  const [status, setStatus] = useState("idle");
  const timeout = useRef();
  useEffect(() => () => clearTimeout(timeout.current), []);
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setStatus("copied");
    } catch {
      setStatus("failed");
    }
    clearTimeout(timeout.current);
    timeout.current = setTimeout(() => setStatus("idle"), 2400);
  }
  return <span className="copy-control">
    <button type="button" className="copy-button" onClick={copy} aria-label={status === "copied" ? "已复制" : `${label}：${text}`} title={label}>
      {status === "copied" ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}<span>{status === "copied" ? "已复制" : label}</span>
    </button>
    <span className="copy-status" role="status">{status === "failed" ? "复制失败，请选中命令复制。" : ""}</span>
  </span>;
}

function GuidedDemo() {
  const [step, setStep] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [visible, setVisible] = useState(false);
  const root = useRef(null);
  useEffect(() => {
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    setPlaying(!preference.matches);
    const onChange = () => setPlaying(!preference.matches);
    preference.addEventListener("change", onChange);
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting), { threshold: 0.4 });
    observer.observe(root.current);
    return () => { observer.disconnect(); preference.removeEventListener("change", onChange); };
  }, []);
  useEffect(() => {
    if (!playing || !visible) return;
    const timer = setInterval(() => setStep(current => (current + 1) % 3), 4500);
    return () => clearInterval(timer);
  }, [playing, visible]);
  const captions = ["按 a，照着提示添加服务器。", "上下选择，按 e 随时编辑。", "按 Enter，连接选中的服务器。"];
  return <div className="guided-demo" ref={root} aria-label="界面操作演示">
    <div className="demo-launch"><code><span>$</span> sshm</code><span>从这一个命令开始</span></div>
    <div className="demo-screen">
      <div className="demo-title"><span>sshm</span><span>操作示意</span></div>
      <div className="demo-body">
        {step === 0 ? <div className="demo-form"><p>添加服务器</p><dl><div><dt>名称</dt><dd>production</dd></div><div><dt>主机</dt><dd>192.0.2.10</dd></div><div><dt>用户</dt><dd>root</dd></div></dl></div> : <div className="demo-list"><p>服务器列表 <span>2 个连接</span></p><table><thead><tr><th>名称</th><th>主机</th></tr></thead><tbody><tr className="demo-selected"><td>production</td><td>192.0.2.10</td></tr><tr><td>staging</td><td>198.51.100.20</td></tr></tbody></table></div>}
        <p className="demo-caption">{captions[step]}</p>
      </div>
      <div className="demo-shortcuts"><span className={step === 0 ? "is-emphasized" : ""}><kbd>a</kbd> 添加</span><span><kbd>e</kbd> 编辑</span><span className={step === 1 ? "is-emphasized" : ""}><kbd>↑↓</kbd> 选择</span><span className={step === 2 ? "is-emphasized" : ""}><kbd>Enter</kbd> 连接</span></div>
    </div>
    <div className="demo-controls"><div className="demo-steps" aria-label="演示步骤">{["添加", "选择", "连接"].map((label, index) => <button key={label} aria-pressed={step === index} onClick={() => { setStep(index); setPlaying(false); }}><span>0{index + 1}</span>{label}</button>)}</div><button className="demo-play" onClick={() => setPlaying(current => !current)} aria-label={playing ? "暂停演示" : "播放演示"}>{playing ? <Pause weight="fill" /> : <Play weight="fill" />}</button></div>
  </div>;
}

function Downloads({ dialogRef }) {
  const [platform, setPlatform] = useState("macos");
  const platforms = [
    { id: "macos", label: "macOS", icon: AppleLogo },
    { id: "windows", label: "Windows", icon: WindowsLogo },
    { id: "linux", label: "Linux", icon: TerminalWindow },
  ];
  const close = () => dialogRef.current?.close();
  function moveTab(event, index) {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const next = event.key === "Home" ? 0 : event.key === "End" ? 2 : (index + (event.key === "ArrowRight" ? 1 : 2)) % 3;
    setPlatform(platforms[next].id);
    document.getElementById(`tab-${platforms[next].id}`)?.focus();
  }
  return <dialog ref={dialogRef} className="download-dialog" aria-labelledby="download-title" onClick={(event) => { if (event.target === event.currentTarget) close(); }}>
    <div className="dialog-content">
      <button className="close-dialog" onClick={close} aria-label="关闭下载窗口"><X size={24} /></button>
      <p className="eyebrow dialog-eyebrow">免费使用，自由连接。</p>
      <h2 id="download-title">选择你的系统。</h2>
      <div className="platform-tabs" role="tablist" aria-label="操作系统">
        {platforms.map(({ id, label, icon: Icon }, index) => <button key={id} id={`tab-${id}`} role="tab" aria-selected={platform === id} aria-controls={`panel-${id}`} tabIndex={platform === id ? 0 : -1} onKeyDown={(event) => moveTab(event, index)} onClick={() => setPlatform(id)}><Icon size={24} aria-hidden="true" />{label}</button>)}
      </div>
      <div className="platform-panel" id={`panel-${platform}`} role="tabpanel" aria-labelledby={`tab-${platform}`}>
        {platform === "macos" && <>

          <div className="download-options">
            <ExternalLink icon={DownloadSimple} className="download-option" href={`${repository}/releases/download/v0.5.0/sshm-0.5.0-macos-aarch64.pkg`}><span><strong>Apple Silicon</strong><small>M 系列芯片 · .pkg</small></span></ExternalLink>
            <ExternalLink icon={DownloadSimple} className="download-option" href={`${repository}/releases/download/v0.5.0/sshm-0.5.0-macos-x86_64.pkg`}><span><strong>Intel</strong><small>Intel 芯片 · .pkg</small></span></ExternalLink>
          </div>

        </>}
        {platform === "windows" && <>

          <div className="download-options"><ExternalLink icon={DownloadSimple} className="download-option" href={`${repository}/releases/download/v0.5.0/sshm-0.5.0-windows-x86_64-local-trial.exe`}><span><strong>获取 Windows 安装包</strong><small>x64 · .exe</small></span></ExternalLink></div>

        </>}
        {platform === "linux" && <>

          <div className="install-command"><code>{installCommand}</code><CopyButton text={installCommand} label="复制命令" /></div>

          <ExternalLink className="text-link small-link" href={`${repository}/blob/main/docs/linux-installation.md`}>查看安装说明</ExternalLink>
        </>}
      </div>
      <div className="dialog-footer">安装后新开终端，输入 <code>sshm</code>。<ExternalLink href={`${repository}/releases`}>所有版本</ExternalLink></div>
    </div>
  </dialog>;
}

export function App() {
  const dialogRef = useRef(null);
  const openDownloads = () => dialogRef.current?.showModal();
  return <>
    <a className="skip-link" href="#main">跳到主要内容</a>
    <div className="page-shell">
      <header className="site-header">
        <a href="#" className="brand-link" aria-label="sshm 首页"><Wordmark /></a>
        <nav aria-label="主导航"><a href="#features">功能</a><button onClick={openDownloads}>安装</button><ExternalLink href={repository}>GitHub</ExternalLink></nav>
      </header>
      <main id="main">
        <section className="hero" aria-labelledby="hero-title">
          <div className="hero-copy">
            <p className="eyebrow">轻量 · 免费 · MIT 开源</p>
            <h1 id="hero-title">轻量的 SSH 管理工具</h1>
            <div className="hero-actions"><button className="primary-button" onClick={openDownloads}>免费下载<ArrowDown size={26} aria-hidden="true" /></button></div>
            <p className="platform-caption">macOS <span>/</span> Windows <span>/</span> Linux</p>
          </div>
          <figure className="terminal-preview"><img src={asset("terminal-demo.png")} alt="sshm 终端连接列表示意：选择 staging 服务器，按 Enter 连接；支持搜索、新增和 Google Drive 同步。" width="1341" height="1173" fetchPriority="high" /></figure>
        </section>
        <section className="features ruled-section" id="features" aria-labelledby="features-title">
          <div className="section-copy"><h2 id="features-title">输入 <code>sshm</code></h2><p>按照提示添加、编辑、连接服务器</p></div>
          <GuidedDemo />
        </section>
        <section className="sync ruled-section" aria-labelledby="sync-title">
          <div className="section-copy"><h2 id="sync-title">配置信息云同步</h2><p>使用你自己的 Google Drive 同步服务器信息</p></div>
          <figure className="sync-illustration"><img src={asset("drive-sync.png")} alt="两台电脑通过自己的 Google Drive 同步服务器连接信息" width="2172" height="724" loading="lazy" /></figure>
        </section>
      </main>
      <footer className="site-footer"><a href="#" className="brand-link" aria-label="返回顶部"><Wordmark /></a><a className="license-link" href={`${repository}/blob/main/LICENSE`} target="_blank" rel="noopener noreferrer">完全免费 · MIT 开源</a><ExternalLink href={repository}>GitHub</ExternalLink></footer>
    </div>
    <Downloads dialogRef={dialogRef} />
  </>;
}
