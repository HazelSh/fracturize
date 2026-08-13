
-------- Go away

**This is pre-release software. We're not done. It's not stable. Development continues.** 

---- I reserve the right to modify without notice:
 - the scene format, making all your existing scenes incompatible
 - the GUI and CLI, and then all your existing workflows will be wrong
 - the renderer, such that old renders are impossible to recreate
 - the math including for variations and coloring, and then your scenes will be subtly incorrect

If this changes, say with a 1.0 public release, I'll edit this readme file and remove the disclaimer. 

-- Additionally, .flam3 compatibility is not a priority for this project.
Converter/importer might turn up at some point, who knows. No promises. 

---- AI code notice
Please note that this project is also 95% vibecoded, mostly by Claudes.
The four main contributors so far are Opus5, Fable5, Opus4.8, and Opus4.5. 
There has been minimal human involvement in the code. If you want a warning, consider yourself warned. 

(have I scared you away yet?)


-------- The Fractal program of my dreams

This project is a fractal editor and renderer, for 3D IFS-derived fractals ("fractal flames").

The goal is to be a solid creative tool and fun digital fidget-toy for myself, and for LLMs I employ. 

I want to out-Apophysis Apophysis, building on a modern and strong technical foundation.
From the start, I wanted 3D, in that same gritty style.


-------- Featuring:

---- Fractals! 3D IFS with 19 different transform variations
 - complex coloring including palette based and structural options
 - full control over variations, up to editing the matricies directly if needed
 - infinite zoom in & out with the scene cycling 'through' an affine transform

---- Full, real 3d, right from the start. 3D native transform variations, no hacks.
 - Fun tweakable UI that represents IFS flames to the user cleanly. Original gizmo design for IFS transforms. 
 - Camera pathing for flybys and fly-throughs, smoothed for easy motion, visualized in the scene.
 - performant, realtime interactivity. Even low end systems can run hundreds of thousands of points while editing.
 - 9 or so variations are 'native 3d' with the rest doing functional passthrough-Z

---- Grainy, gritty, sandy, dusty renders with plenty of spots and noise. Pre-fuzzed for your viewing pleasure.
 - (yes I like the Apophysis house style)

---- Agent-first CLI for all features. Feature-compatible with the human-first GUI. 
 - CLI scene authoring, metrics, tweaking, parameter sweeps, scene mutations, etc. 

---- Modern technical foundation
 - Project is in Rust, with WGPU and EGUI. Linux first.
 - Native hardware acceleration from the start. 

---- Advanced rendering with batch jobs
 - including animated renders
 - drop 'view' files for later rendering

---- complex scene metrics
 - realtime calculation of fractal-dimension, contraction, and lacunarity for authoring guidance

-------- endnote

Anyway. I think it's fun. If you're reading this, you can try it out too. Make something neat!

Hazel (partially on behalf of assorted Claudes)


