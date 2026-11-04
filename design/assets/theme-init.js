/* Runs before first paint: theme and effect level (weak devices get data-fx="low"). */
(function(){var r=document.documentElement,s=localStorage,t=null,f=null;try{t=s.getItem("gk-theme");f=s.getItem("gk-fx")}catch(e){}
if(!t)t=matchMedia("(prefers-color-scheme: light)").matches?"light":"dark";r.setAttribute("data-theme",t);
var n=navigator,c=n.connection||{},weak=(n.hardwareConcurrency&&n.hardwareConcurrency<=2)||(n.deviceMemory&&n.deviceMemory<=2)||c.saveData||matchMedia("(prefers-reduced-transparency: reduce)").matches;
r.setAttribute("data-fx",f||(weak?"low":"high"))})();
