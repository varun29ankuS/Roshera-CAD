(function(){
  "use strict";
  // ---- reveal on scroll ----

  // ---------- pipe flange, OD 50, chamfered ----------
  // Dimensions are the real part: OD50 x 6 plate, O26 hub 13 overall,
  // O16 bore, 4 x O5 on O38 PCD, 0.8 x 45 chamfers. 23 analytic faces.
  var RO=25, RHUB=13, RBORE=8, RPCD=19, RHOLE=2.5, C=0.8;
  var TP=6, HT=13, RCH=RHOLE+C;              // plate top, hub top, hole+chamfer
  var MID=6.5;                                // vertical centring offset
  var CHAM=[.86,.57,.30], CYLC=[.40,.50,.60], PLAN=[.60,.65,.69];

  var POS=[],NRM=[],COL=[],IDX=[];
  function vert(x,y,z,nx,ny,nz,c){
    POS.push(x,y-MID,z); NRM.push(nx,ny,nz); COL.push(c[0],c[1],c[2]);
    return (POS.length/3)-1;
  }
  // Band of revolution between (r0,h0) and (r1,h1) about a vertical axis at (cx,cz).
  // Traversal order sets the normal: it is the profile tangent rotated -90 deg.
  function band(cx,cz,r0,h0,r1,h1,c,seg){
    var dr=r1-r0, dh=h1-h0, L=Math.hypot(dr,dh)||1, nr=dh/L, nh=-dr/L, j, base=POS.length/3;
    for(j=0;j<=seg;j++){
      var a=j/seg*Math.PI*2, ca=Math.cos(a), sa=Math.sin(a);
      vert(cx+r0*ca, h0, cz+r0*sa, nr*ca, nh, nr*sa, c);
      vert(cx+r1*ca, h1, cz+r1*sa, nr*ca, nh, nr*sa, c);
    }
    for(j=0;j<seg;j++){
      var p=base+j*2;
      IDX.push(p,p+1,p+2, p+1,p+3,p+2);
    }
  }
  // --- flat annular face, holes solved analytically ---
  // For each angle the ray from the axis meets the nearest bolt hole at
  // r = pcd*cos(phi) +/- sqrt(hr^2 - (pcd*sin(phi))^2). Outside the hole's angular
  // span both roots collapse to pcd*cos(phi), so the face is always emitted as two
  // radial strips that meet with no seam and no T-junction. No triangulator, no gaps.
  function angDiff(a,b){var d=a-b;while(d>Math.PI)d-=2*Math.PI;while(d<-Math.PI)d+=2*Math.PI;return d;}
  function splitRadii(th,Ri,Ro,ho){
    if(!ho)return [Ri,Ri];
    var best=0,bd=9e9,i,d;
    for(i=0;i<ho.angles.length;i++){
      d=Math.abs(angDiff(th,ho.angles[i]*Math.PI/180));
      if(d<bd){bd=d;best=i;}
    }
    var phi=angDiff(th,ho.angles[best]*Math.PI/180);
    var s=ho.pcd*Math.sin(phi), cc=ho.pcd*Math.cos(phi), lo, hi;
    if(Math.abs(s)<ho.r){var q=Math.sqrt(ho.r*ho.r-s*s);lo=cc-q;hi=cc+q;}
    else {lo=hi=cc;}
    return [Math.min(Math.max(lo,Ri),Ro), Math.min(Math.max(hi,Ri),Ro)];
  }
  function annulus(h,Ri,Ro,ny,c,ho){
    var N=720,j,base=POS.length/3;
    for(j=0;j<=N;j++){
      var th=j/N*Math.PI*2, ca=Math.cos(th), sa=Math.sin(th);
      var sp=splitRadii(th,Ri,Ro,ho);
      vert(Ri*ca,h,Ri*sa, 0,ny,0, c);
      vert(sp[0]*ca,h,sp[0]*sa, 0,ny,0, c);
      vert(sp[1]*ca,h,sp[1]*sa, 0,ny,0, c);
      vert(Ro*ca,h,Ro*sa, 0,ny,0, c);
    }
    for(j=0;j<N;j++){
      var p=base+j*4, n=p+4;
      IDX.push(p,p+1,n, p+1,n+1,n);        // inner strip: Ri -> lo
      IDX.push(p+2,p+3,n+2, p+3,n+3,n+2);  // outer strip: hi -> Ro
    }
  }

  function build(){
    var S=120, i, bolts=[], ang=[0,90,180,270];
    ang.forEach(function(d){
      var a=d*Math.PI/180;
      bolts.push({x:RPCD*Math.cos(a), z:RPCD*Math.sin(a), deg:d});
    });
    // --- axisymmetric bands ---
    band(0,0, RO-C,0,      RO,C,        CHAM,S);   // bottom outer chamfer
    band(0,0, RO,C,        RO,TP-C,     CYLC,S);   // outer wall
    band(0,0, RO,TP-C,     RO-C,TP,     CHAM,S);   // top outer chamfer
    band(0,0, RHUB,TP,     RHUB,HT-C,   CYLC,S);   // hub wall
    band(0,0, RHUB,HT-C,   RHUB-C,HT,   CHAM,S);   // hub top chamfer
    band(0,0, RBORE+C,HT,  RBORE,HT-C,  CHAM,S);   // bore top chamfer
    band(0,0, RBORE,HT-C,  RBORE,C,     CYLC,S);   // bore wall
    band(0,0, RBORE,C,     RBORE+C,0,   CHAM,S);   // bore bottom chamfer
    // --- bolt holes: chamfer, wall, chamfer ---
    bolts.forEach(function(b){
      band(b.x,b.z, RCH,TP,   RHOLE,TP-C, CHAM,48);
      band(b.x,b.z, RHOLE,TP-C, RHOLE,C,  CYLC,48);
      band(b.x,b.z, RHOLE,C,  RCH,0,      CHAM,48);
    });
    // --- flat faces ---
    var HO={pcd:RPCD, r:RCH, angles:[0,90,180,270]};
    annulus(0,  RBORE+C, RO-C,   -1, PLAN, HO);   // underside
    annulus(TP, RHUB,    RO-C,    1, PLAN, HO);   // plate top
    annulus(HT, RBORE+C, RHUB-C,  1, PLAN, null); // hub top

    return {pos:new Float32Array(POS),nrm:new Float32Array(NRM),
            col:new Float32Array(COL),idx:new Uint32Array(IDX)};
  }

  var cv=document.getElementById("gl"), fb=document.getElementById("fb");
  var gl=null;
  try{ gl=cv.getContext("webgl",{antialias:true,alpha:true})||cv.getContext("experimental-webgl"); }catch(e){}
  var ext=gl&&gl.getExtension("OES_element_index_uint");
  if(!gl||!ext){ cv.style.display="none"; if(fb) fb.style.display="block"; return; }

  var VS=
    "attribute vec3 aP;attribute vec3 aN;attribute vec3 aC;"+
    "uniform mat4 uMVP;uniform mat4 uM;varying vec3 vN;varying vec3 vC;varying vec3 vP;"+
    "void main(){vN=mat3(uM)*aN;vC=aC;vec4 w=uM*vec4(aP,1.);vP=w.xyz;gl_Position=uMVP*vec4(aP,1.);}";
  var FS=
    "precision mediump float;varying vec3 vN;varying vec3 vC;varying vec3 vP;"+
    "uniform float uDark;"+
    "void main(){vec3 N=normalize(vN);vec3 V=normalize(vec3(0.,0.,1.));"+
    "if(!gl_FrontFacing)N=-N;"+
    "vec3 L1=normalize(vec3(.55,.75,.65));vec3 L2=normalize(vec3(-.6,.2,.4));"+
    "float d1=max(dot(N,L1),0.),d2=max(dot(N,L2),0.)*.42;"+
    "vec3 H=normalize(L1+V);float sp=pow(max(dot(N,H),0.),42.)*.5;"+
    "float rim=pow(1.-max(dot(N,V),0.),2.6)*.34;"+
    "vec3 amb=mix(vec3(.34),vec3(.16),uDark);"+
    "vec3 c=vC*(amb+d1*.90+d2)+vec3(sp)*.7+vC*rim;"+
    "gl_FragColor=vec4(clamp(c,0.,1.),1.);}";
  function sh(t,s){var o=gl.createShader(t);gl.shaderSource(o,s);gl.compileShader(o);return o;}
  var pr=gl.createProgram();
  gl.attachShader(pr,sh(gl.VERTEX_SHADER,VS));gl.attachShader(pr,sh(gl.FRAGMENT_SHADER,FS));
  gl.linkProgram(pr);
  if(!gl.getProgramParameter(pr,gl.LINK_STATUS)){cv.style.display="none";if(fb)fb.style.display="block";return;}
  gl.useProgram(pr);

  var G=build();
  function buf(data,loc,n){
    var b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);
    gl.bufferData(gl.ARRAY_BUFFER,data,gl.STATIC_DRAW);
    var l=gl.getAttribLocation(pr,loc);gl.enableVertexAttribArray(l);
    gl.vertexAttribPointer(l,n,gl.FLOAT,false,0,0);
  }
  buf(G.pos,"aP",3);buf(G.nrm,"aN",3);buf(G.col,"aC",3);
  var ib=gl.createBuffer();
  gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER,ib);
  gl.bufferData(gl.ELEMENT_ARRAY_BUFFER,G.idx,gl.STATIC_DRAW);

  var uMVP=gl.getUniformLocation(pr,"uMVP"),uM=gl.getUniformLocation(pr,"uM"),
      uDark=gl.getUniformLocation(pr,"uDark");
  gl.enable(gl.DEPTH_TEST);

  function mul(a,b){var o=new Float32Array(16),i,j,k;
    for(i=0;i<4;i++)for(j=0;j<4;j++){var s=0;for(k=0;k<4;k++)s+=a[k*4+j]*b[i*4+k];o[i*4+j]=s;}return o;}
  function persp(f,ar,n,fa){var t=1/Math.tan(f/2);return new Float32Array(
    [t/ar,0,0,0, 0,t,0,0, 0,0,(fa+n)/(n-fa),-1, 0,0,2*fa*n/(n-fa),0]);}
  function rotY(a){var c=Math.cos(a),s=Math.sin(a);
    return new Float32Array([c,0,-s,0, 0,1,0,0, s,0,c,0, 0,0,0,1]);}
  function rotX(a){var c=Math.cos(a),s=Math.sin(a);
    return new Float32Array([1,0,0,0, 0,c,s,0, 0,-s,c,0, 0,0,0,1]);}
  function trans(x,y,z){return new Float32Array([1,0,0,0,0,1,0,0,0,0,1,0,x,y,z,1]);}

  var ry=-0.5, rx=0.62, drag=false, lx=0, ly=0, auto=true;
  function down(e){drag=true;auto=false;var t=e.touches?e.touches[0]:e;lx=t.clientX;ly=t.clientY;}
  function move(e){
    if(!drag)return; if(e.cancelable)e.preventDefault();
    var t=e.touches?e.touches[0]:e;
    ry+=(t.clientX-lx)*.0095; rx+=(t.clientY-ly)*.0075;
    rx=Math.max(-1.35,Math.min(1.35,rx)); lx=t.clientX; ly=t.clientY;
  }
  function up(){drag=false;}
  cv.addEventListener("mousedown",down);window.addEventListener("mousemove",move);
  window.addEventListener("mouseup",up);
  cv.addEventListener("touchstart",down,{passive:true});
  cv.addEventListener("touchmove",move,{passive:false});
  window.addEventListener("touchend",up);

  var reduce=window.matchMedia&&window.matchMedia("(prefers-reduced-motion:reduce)").matches;
  // Paper is the default world; only an explicit toggle darkens the viewer.
  function isDark(){
    return document.documentElement.getAttribute("data-theme")==="dark"?1:0;
  }
  function frame(){
    var dpr=Math.min(window.devicePixelRatio||1,2);
    var w=cv.clientWidth,h=cv.clientHeight;
    if(cv.width!==w*dpr||cv.height!==h*dpr){cv.width=w*dpr;cv.height=h*dpr;}
    gl.viewport(0,0,cv.width,cv.height);
    gl.clearColor(0,0,0,0);gl.clear(gl.COLOR_BUFFER_BIT|gl.DEPTH_BUFFER_BIT);
    if(auto&&!reduce)ry+=.0032;
    var M=mul(rotX(rx),rotY(ry));
    var V=trans(0,0,-78);
    var P=persp(.70,(cv.width/cv.height)||1,1,600);
    gl.uniformMatrix4fv(uM,false,M);
    gl.uniformMatrix4fv(uMVP,false,mul(P,mul(V,M)));
    gl.uniform1f(uDark,isDark());
    gl.drawElements(gl.TRIANGLES,G.idx.length,gl.UNSIGNED_INT,0);
    requestAnimationFrame(frame);
  }
  frame();
})();
