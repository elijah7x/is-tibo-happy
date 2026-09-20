// is-tibo-happy 注入侧：账号菜单里的小卡片。
// 注入方式：Node 侧把本文件文本经 Runtime.evaluate 执行，挂 window.__ith。
// 卡片位置：用户名区第一个分隔线之后（账号信息区，不切原生功能组）。
window.__ith = (function () {
  if (window.__ith?.destroy) window.__ith.destroy();

  const COLORS = {
    happy: '#ff3b30',      // Apple systemRed：配头像红心
    unhappy: '#8e8e93',    // Apple systemGray
    offline: null,
  };
  const WORD = { happy: 'HAPPY', unhappy: 'UNHAPPY', offline: 'OFFLINE' };
  const VARIANT_FOR = { happy: 'happy', unhappy: 'unhappy', offline: 'unhappy' };

  // 内置占位头像（8x8 抽象脸），setAvatar 后被真图替换
  const PLACEHOLDER = {
    grid: 8,
    palette: { a: '#5b5f6b', b: '#9aa0ad', c: '#d6d9de', '.': 'transparent' },
    variants: {
      happy:   ['........', '.aabbaa.', '.abcbaa.', '.aabbaa.', '.abbbba.', '.acccca.', '..aaaa..', '........'],
      unhappy: ['........', '.aabbaa.', '.abcaba.', '.aabbaa.', '.abbbba.', '.acccca.', '..aaaa..', '........'],
    },
  };

  let mo = null, card = null;
  let avatar = PLACEHOLDER;
  let state = { kind: 'unhappy', detail: {} };   // 两态口径：首个 setState 前显示 UNHAPPY+暂无预告

  // 语言跟随界面：只认 navigator.language（App 语言跟随系统；触发器 label 判语言会误伤 ja）
  function detectLang() {
    return (navigator.language || '').toLowerCase().startsWith('zh') ? 'zh' : 'en';
  }
  const LOCAL = {   // detail.sub 缺席时的兜底（daemon 除 offline 外必带 sub）
    zh: { none: '暂无重置预告', offline: '数据不可用' },
    en: { none: 'no reset news', offline: 'data unavailable' },
  };

  function subLine(kind, detail) {
    const lang = detectLang();
    if (detail?.sub) {
      if (typeof detail.sub === 'string') return detail.sub;
      return detail.sub[lang] || detail.sub.en || detail.sub.zh || '';
    }
    return kind === 'offline' ? LOCAL[lang].offline : LOCAL[lang].none;
  }

  function isProfileMenu(menu) {
    const by = menu.getAttribute('aria-labelledby');
    if (!by) return false;
    const trig = document.getElementById(by);
    return trig && !!trig.closest('aside') && trig.getAttribute('aria-haspopup') === 'menu'
      && /prof|perfil|próf|προφ|проф|پروفا|نمایه|الملف|प्रोफ|ਪ੍ਰੋ|પ્રો|ಪ್ರೊ|പ്രൊ|ప్రొ|প্রোফাইল|சுயவிவர|პროფ|պրոֆ|โปรไฟล์|hồ sơ|wasifu|aqoonsiga|መገለጫ|ပရိုဖိုင်|プロフィール|프로필|个人资料|個人檔案|资料|檔案|账号|帳號|account|compte|cuenta|conta|アカウント|계정/i
        .test(trig.getAttribute('aria-label') || '');
  }

  // canvas 像素渲染：grid×grid 数据 → 固定 ~48px 方块（fillRect 微秒级，无巨型 CSS 串）
  function renderAvatar(kind, size = 48) {
    const rows = avatar.variants[VARIANT_FOR[kind] || 'happy'] || [];
    const grid = avatar.grid || rows.length || 8;
    const cv = document.createElement('canvas');
    cv.width = grid; cv.height = grid;
    cv.style.cssText = `width:${size}px;height:${size}px;flex:none;` +
      'border-radius:6px;image-rendering:pixelated;' +
      'box-shadow:inset 0 0 0 .5px rgba(0,0,0,.15);' +
      (kind === 'offline' ? 'filter:grayscale(1);opacity:.45;' : '');
    const ctx = cv.getContext('2d');
    for (let y = 0; y < rows.length; y++) {
      for (let x = 0; x < rows[y].length; x++) {
        const col = avatar.palette[rows[y][x]];
        if (col && col !== 'transparent') {
          ctx.fillStyle = col;
          ctx.fillRect(x, y, 1, 1);
        }
      }
    }
    return cv;
  }

  function makeCard(menu) {
    const ref = menu?.querySelector('[role="menuitem"]') || document.createElement('div');
    const cs = getComputedStyle(ref);
    const el = document.createElement('div');
    el.setAttribute('data-ith', '');
    el.setAttribute('role', 'presentation');
    el.setAttribute('aria-hidden', 'true');
    el.style.cssText = 'display:flex;align-items:center;gap:10px;min-width:0;' +
      `padding:${cs.padding};margin:2px 0 4px;border-radius:${cs.borderRadius};` +
      `font-family:${cs.fontFamily};color:${cs.color};`;

    const txt = document.createElement('div');
    txt.style.cssText = `display:flex;flex-direction:column;min-width:0;line-height:${cs.lineHeight};`;
    const l1 = document.createElement('div');
    l1.style.cssText = `font-size:${cs.fontSize};font-weight:600;white-space:nowrap;`;
    l1.append('Tibo ');
    const word = document.createElement('span');
    word.textContent = WORD[state.kind] || String(state.kind || 'TIBO').toUpperCase();
    if (COLORS[state.kind]) word.style.color = COLORS[state.kind];
    if (state.kind === 'offline') word.style.opacity = '.5';
    l1.appendChild(word);
    const l2 = document.createElement('div');
    l2.textContent = subLine(state.kind, state.detail);
    l2.style.cssText = `font-size:calc(${cs.fontSize} - 1.5px);opacity:.55;white-space:nowrap;`;
    txt.append(l1, l2);
    el.append(renderAvatar(state.kind), txt);
    return el;
  }

  function mount(menu) {
    if (card || !menu.isConnected) return;
    card = makeCard(menu);
    const inner = menu.firstElementChild || menu;
    const kids = [...inner.children];
    // 用户名后第一个分隔线（空文本、非 menuitem）之后
    const sep = kids.find(e => e.getAttribute('role') !== 'menuitem' && (e.textContent || '').trim() === '');
    if (sep && sep.nextSibling) inner.insertBefore(card, sep.nextSibling);
    else if (kids[1]) inner.insertBefore(card, kids[1]);
    else inner.appendChild(card);
    if (!card.isConnected) { card = null; console.warn('[ith] menu anchor not found'); }
    else { try { window.ithRefresh && window.ithRefresh(''); } catch {} }   // 通知守护进程节流刷新
  }

  function unmount() { card?.remove(); card = null; }

  let everMatched = false, reportedUnmatched = false;   // 账号菜单认出过就永不上报；否则每次注入只报一次
  function scan() {
    let sawMenu = false;
    for (const m of document.querySelectorAll('[data-radix-menu-content][role="menu"]')) {
      sawMenu = true;
      if (isProfileMenu(m)) { everMatched = true; mount(m); return; }
    }
    // 从未认出账号菜单、却见到 aside 弹层 → 上报 daemon（大概率是 label 文案又变了）
    if (sawMenu && !everMatched && !reportedUnmatched) {
      const m = document.querySelector('[data-radix-menu-content][role="menu"]');
      const trig = document.getElementById(m?.getAttribute('aria-labelledby') || '');
      if (trig?.closest('aside')) {
        reportedUnmatched = true;
        try { window.ithRefresh?.('menu-unmatched:' + (trig.getAttribute('aria-label') || '').slice(0, 60)); } catch {}
      }
    }
    if (card && !card.isConnected) card = null;
  }

  // 只在新增节点可能含菜单时才全文档扫；纯删除/文本变更只做卡片存活检查
  mo = new MutationObserver(muts => {
    if (card && !card.isConnected) card = null;
    for (const m of muts) {
      for (const n of m.addedNodes) {
        if (n.nodeType !== 1) continue;
        if (n.matches?.('[data-radix-menu-content]') ||
            n.querySelector?.('[data-radix-menu-content]')) { scan(); return; }
      }
    }
  });
  mo.observe(document.body, { childList: true, subtree: true });
  scan();

  return {
    setState(s) {
      if (!s || !s.kind) return;
      s.detail ||= {};
      if (JSON.stringify(s) === JSON.stringify(state)) return;
      state = s;
      if (card) { unmount(); scan(); }
    },
    setAvatar(a) { if (a?.variants) avatar = a; if (card) { unmount(); scan(); } },
    force(kind) { this.setState({ kind, detail: { daysSince: kind === 'unhappy' ? 4.5 : null, scheduledISO: kind === 'happy' ? new Date(Date.now() + 7200e3).toISOString() : null } }); },
    destroy() { mo?.disconnect(); unmount(); window.__ith = null; },
  };
})();
'installed';
