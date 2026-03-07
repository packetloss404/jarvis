use crate::app_state::core::JarvisApp;

impl JarvisApp {
    pub(super) fn toggle_blank_for_focused_pane(&mut self) {
        let pane_id = self.tiling.focused_id();
        let should_blank = !self.blanked_panes.contains(&pane_id);
        self.set_pane_blanked(pane_id, should_blank);
    }

    pub(super) fn apply_blank_state_to_pane(&self, pane_id: u32) {
        let Some(registry) = &self.webviews else {
            return;
        };
        let Some(handle) = registry.get(pane_id) else {
            return;
        };

        let blanked = self.blanked_panes.contains(&pane_id);
        let _ = handle.evaluate_script(&blank_overlay_script(blanked));
    }

    fn set_pane_blanked(&mut self, pane_id: u32, blanked: bool) {
        if blanked {
            self.blanked_panes.insert(pane_id);
        } else {
            self.blanked_panes.remove(&pane_id);
        }
        self.apply_blank_state_to_pane(pane_id);
    }
}

fn blank_overlay_script(blanked: bool) -> String {
    let script = r#"(function(){
var overlayId='_jv_blank_overlay';
var styleId='_jv_blank_overlay_style';
var blockKeydown=window._jvBlankBlockKeydown;
var blockKeyup=window._jvBlankBlockKeyup;
if(!blockKeydown){
blockKeydown=function(e){
if(!document.getElementById(overlayId)||document.getElementById('_cp_overlay'))return;
e.preventDefault();
e.stopPropagation();
};
blockKeyup=function(e){
if(!document.getElementById(overlayId)||document.getElementById('_cp_overlay'))return;
e.preventDefault();
e.stopPropagation();
};
window._jvBlankBlockKeydown=blockKeydown;
window._jvBlankBlockKeyup=blockKeyup;
}
if(__BLANKED__){
var root=document.head||document.documentElement;
if(root&&!document.getElementById(styleId)){
var style=document.createElement('style');
style.id=styleId;
style.textContent='#_jv_blank_overlay{position:fixed;inset:0;background:#000;z-index:99999;pointer-events:auto;cursor:none;}';
root.appendChild(style);
}
if(document.body&&!document.getElementById(overlayId)){
var overlay=document.createElement('div');
overlay.id=overlayId;
overlay.setAttribute('aria-hidden','true');
overlay.addEventListener('mousedown',function(e){e.preventDefault();e.stopPropagation();});
overlay.addEventListener('mouseup',function(e){e.preventDefault();e.stopPropagation();});
overlay.addEventListener('click',function(e){e.preventDefault();e.stopPropagation();});
document.body.appendChild(overlay);
}
document.addEventListener('keydown',blockKeydown,true);
document.addEventListener('keyup',blockKeyup,true);
}else{
var overlay=document.getElementById(overlayId);
if(overlay)overlay.remove();
document.removeEventListener('keydown',blockKeydown,true);
document.removeEventListener('keyup',blockKeyup,true);
}
})();"#;

    script.replace("__BLANKED__", if blanked { "true" } else { "false" })
}
