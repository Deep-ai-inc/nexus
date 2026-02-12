// ==========================================================================
// Flicker-Free macOS Metal Window Resize — Reference Implementation
// ==========================================================================
//
// Build: clang -framework Cocoa -framework Metal -framework QuartzCore
//        -framework IOSurface -framework CoreVideo -o resize-demo main.m
// Run:   ./resize-demo
//
// ==========================================================================
// THE PROBLEM
// ==========================================================================
//
// macOS window resize is notoriously difficult with Metal/GPU-rendered content.
// During a live resize drag, the Window Server enters a special "tracking loop"
// that imposes severe constraints on what actually reaches the screen:
//
//   1. CA transactions don't commit. Any property change on a CALayer
//      (contents, opacity, etc.) is batched into an implicit CATransaction
//      that only flushes when the tracking loop yields — which only happens
//      on geometry changes (mouse movement). This means:
//        - presentsWithTransaction freezes content indefinitely
//        - Setting layer.contents directly is invisible until the next
//          mouse move event
//        - [CATransaction flush] has no effect during tracking
//
//   2. presentDrawable on a hidden layer strands drawables. If you hide a
//      CAMetalLayer while drawables are in the presentation pipeline, those
//      drawables never recycle. After 2-3 such events, nextDrawable blocks
//      permanently (1s timeout with allowsNextDrawableTimeout, infinite
//      without).
//
//   3. dispatch_async to main queue doesn't execute when the mouse is still
//      during a resize drag. The main run loop is waiting for the next event
//      and doesn't drain the GCD source.
//
//   4. Sublayers cover root layer content. Setting IOSurface on the root
//      CALayer is invisible if a CAMetalLayer sublayer sits on top showing
//      its last (frozen) frame.
//
//   5. setContents on CAMetalLayer is undefined behavior. Apple explicitly
//      warns against it. It corrupts the drawable system permanently —
//      presentDrawable stops working for the lifetime of the layer.
//
// ==========================================================================
// THE SOLUTION: Three-Layer Architecture + Dual-Mode Rendering
// ==========================================================================
//
// Layer hierarchy (bottom to top):
//   - Root CALayer (view's backing layer) — background color only
//   - CAMetalLayer (sublayer) — normal rendering via presentDrawable
//   - Overlay CALayer (sublayer) — IOSurface display during active resize
//
// Two rendering modes, switched based on resize state:
//
// MODE 1: Active Drag (setFrameSize is firing)
//   - Triggered by: setFrameSize: during live resize
//   - Renders to: CAMetalLayer drawable (for the texture/IOSurface)
//   - Displays via: overlay CALayer .contents = IOSurface
//   - Key details:
//     * Get IOSurface from drawable.texture BEFORE presentDrawable
//       (accessing after present is undefined)
//     * Do NOT call presentDrawable — drawables don't recycle during
//       tracking loop, leading to exhaustion and 1s freezes
//     * commit + waitUntilCompleted to ensure GPU finishes before we
//       hand the IOSurface to the overlay layer
//     * Overlay layer is shown (hidden = NO), covering the sublayer
//     * The overlay's IOSurface update is visible because setFrameSize
//       triggers a geometry change, which flushes the CA transaction
//
// MODE 2: Mouse Idle (timer fires) + Normal Operation
//   - Triggered by: NSTimer (during resize, mouse still) or CVDisplayLink
//   - Renders to: CAMetalLayer drawable
//   - Displays via: normal presentDrawable on the CAMetalLayer sublayer
//   - Key details:
//     * Overlay is hidden, sublayer is fully visible
//     * Standard non-blocking GPU pipeline with semaphore backpressure
//     * presentDrawable works when mouse is still because Window Server
//       only blocks layer updates during active geometry changes
//     * NSTimer on NSRunLoopCommonModes fires during resize tracking
//       even when the mouse is still (unlike dispatch_async)
//
// Mode transition (drag → idle):
//   - Timer fires after one display refresh interval of no setFrameSize calls
//   - Hides overlay, clears its contents, renders via presentDrawable
//   - No drawable stranding because CAMetalLayer is never hidden
//
// Mode transition (idle → drag):
//   - setFrameSize fires, shows overlay, renders sync IOSurface
//   - Resets the idle timer
//   - CAMetalLayer sublayer keeps its last presentDrawable frame
//     (harmless — overlay covers it)
//
// ==========================================================================
// DEAD ENDS (approaches that DON'T work)
// ==========================================================================
//
// We tried all of these before arriving at the solution above:
//
// - presentsWithTransaction = YES
//   Content freezes indefinitely when mouse is still during drag.
//   CA transactions never commit during the tracking loop.
//
// - IOSurface on root CALayer (no overlay)
//   Invisible — the CAMetalLayer sublayer sits on top, showing its
//   frozen last frame. Root layer contents are painted behind sublayers.
//
// - Hiding the CAMetalLayer sublayer during resize
//   Strands in-flight drawables. After a few hide/unhide cycles,
//   nextDrawable blocks permanently.
//
// - [CATransaction flush] during tracking loop
//   No effect. The Window Server's tracking mode ignores explicit flushes.
//
// - dispatch_async to main queue for timer-like behavior
//   Blocks don't execute when mouse is still during tracking.
//
// - setContents: on CAMetalLayer directly
//   "Changing contents on CAMetalLayer may result in undefined behavior."
//   Confirmed: corrupts the drawable system, presentDrawable stops working.
//
// - drawRect: / updateLayer path
//   updateLayer IS called during resize, but setting layer.contents in it
//   still doesn't flush to screen (same CA transaction issue). Only works
//   for actual CG drawing into the layer's backing store.
//
// - Promoting CAMetalLayer to root layer (via setLayer:)
//   Breaks wgpu-hal's WgpuObserverLayer KVO system. The layer loses
//   geometry tracking and stops receiving bounds/contentsScale updates.
//
// ==========================================================================
// PERFORMANCE CHARACTERISTICS
// ==========================================================================
//
// - Normal rendering: 120Hz on background CVDisplayLink thread
// - Resize rendering: Matches mouse event rate (~120Hz on ProMotion)
// - Input latency: Double-buffered semaphore (kMaxInFlightFrameCount = 2)
//   for minimum latency (1 frame ahead vs 2 with triple buffering)
// - Drawable pool: 3 drawables (extra headroom for sync/async transitions)
// - Color: Explicit sRGB colorspace for correct rendering on P3 displays
// - Thread safety: @synchronized protects drawableSize during resize
// - Backpressure: dispatch_semaphore prevents CPU from outrunning GPU
//
// ==========================================================================

#import <Cocoa/Cocoa.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#import <IOSurface/IOSurface.h>

// Double buffering: lowest latency (1 frame ahead).
// Triple buffering (3) would add ~8ms latency but smooth over micro-stutters.
static const NSUInteger kMaxInFlightFrameCount = 2;

@interface ResizeView : NSView
@property (nonatomic, strong) CAMetalLayer *metalLayer;
@property (nonatomic, strong) CALayer *overlayLayer;
@property (nonatomic, strong) id<MTLDevice> device;
@property (nonatomic, strong) id<MTLCommandQueue> commandQueue;
@property (nonatomic, strong) id<MTLRenderPipelineState> pipeline;

@property (nonatomic, strong) dispatch_semaphore_t inFlightSemaphore;
@property (nonatomic, assign) CFTimeInterval startTime;
@property (nonatomic, assign) CVDisplayLinkRef displayLink;

@property (nonatomic, assign) BOOL isResizing;
@property (nonatomic, strong) NSTimer *resizeTimer;
@property (nonatomic, assign) NSTimeInterval displayRefreshInterval;
@end

@implementation ResizeView

- (instancetype)initWithFrame:(NSRect)frame {
    self = [super initWithFrame:frame];
    if (self) {
        self.wantsLayer = YES;
        self.layerContentsRedrawPolicy = NSViewLayerContentsRedrawOnSetNeedsDisplay;

        _device = MTLCreateSystemDefaultDevice();
        _commandQueue = [_device newCommandQueue];
        _inFlightSemaphore = dispatch_semaphore_create(kMaxInFlightFrameCount);

        // Animated grid shader.
        NSString *src = @
            "#include <metal_stdlib>\n"
            "using namespace metal;\n"
            "struct V { float4 pos [[position]]; };\n"
            "vertex V vs(uint vid [[vertex_id]]) {\n"
            "  V o; float2 p = float2((vid&1)*4.0-1.0, (vid&2)*2.0-1.0);\n"
            "  o.pos = float4(p,0,1); return o;\n"
            "}\n"
            "fragment float4 fs(V in [[stage_in]], constant float &time [[buffer(0)]]) {\n"
            "  float2 p = in.pos.xy + float2(time * 60.0, time * 40.0);\n"
            "  float g = step(0.5, fract(p.x/40.0)) + step(0.5, fract(p.y/40.0));\n"
            "  float c = (fmod(g,2.0) == 0.0) ? 0.2 : 0.35;\n"
            "  return float4(c, c, c+0.1, 1.0);\n"
            "}\n";
        NSError *err = nil;
        id<MTLLibrary> lib = [_device newLibraryWithSource:src options:nil error:&err];
        MTLRenderPipelineDescriptor *pd = [[MTLRenderPipelineDescriptor alloc] init];
        pd.vertexFunction = [lib newFunctionWithName:@"vs"];
        pd.fragmentFunction = [lib newFunctionWithName:@"fs"];
        pd.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
        _pipeline = [_device newRenderPipelineStateWithDescriptor:pd error:&err];

        // Disable implicit animations.
        NSDictionary *noAnim = @{
            @"bounds": [NSNull null], @"position": [NSNull null],
            @"contents": [NSNull null], @"hidden": [NSNull null],
        };

        // CAMetalLayer as sublayer (same as wgpu-hal default).
        _metalLayer = [CAMetalLayer layer];
        _metalLayer.device = _device;
        _metalLayer.pixelFormat = MTLPixelFormatBGRA8Unorm;
        _metalLayer.framebufferOnly = YES;
        _metalLayer.allowsNextDrawableTimeout = YES;
        _metalLayer.presentsWithTransaction = NO;
        // 3 drawables even with semaphore=2: the extra drawable prevents
        // stalls when transitioning between sync (resize) and async (normal)
        // rendering modes. With only 2, mode transitions can deadlock.
        _metalLayer.maximumDrawableCount = 3;
        _metalLayer.colorspace = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        _metalLayer.actions = noAnim;
        _metalLayer.contentsGravity = kCAGravityTopLeft;

        // Overlay layer above metalLayer for IOSurface during resize.
        _overlayLayer = [CALayer layer];
        _overlayLayer.actions = noAnim;
        _overlayLayer.contentsGravity = kCAGravityTopLeft;
        _overlayLayer.hidden = YES;

        [self.layer addSublayer:_metalLayer];
        [self.layer addSublayer:_overlayLayer];
        self.layer.actions = noAnim;
        self.layer.backgroundColor = CGColorCreateGenericRGB(0.15, 0.15, 0.15, 1.0);

        _metalLayer.frame = self.layer.bounds;
        _metalLayer.contentsScale = self.layer.contentsScale;
        _overlayLayer.frame = self.layer.bounds;
        _overlayLayer.contentsScale = self.layer.contentsScale;

        _startTime = CACurrentMediaTime();

        // CVDisplayLink fires on a high-priority background thread at monitor
        // refresh rate. We render directly on this thread (no dispatch_async
        // to main queue) for lowest jitter. Paused during live resize.
        CVDisplayLinkCreateWithActiveCGDisplays(&_displayLink);
        CVDisplayLinkSetOutputCallback(_displayLink, &displayLinkCallback, (__bridge void *)self);

        // Query display refresh rate for adaptive timer interval.
        CVTime period = CVDisplayLinkGetNominalOutputVideoRefreshPeriod(_displayLink);
        if (period.flags & kCVTimeIsIndefinite) {
            _displayRefreshInterval = 1.0 / 60.0;  // fallback
        } else {
            _displayRefreshInterval = (double)period.timeValue / (double)period.timeScale;
        }

        CVDisplayLinkStart(_displayLink);
    }
    return self;
}

// CVDisplayLink callback — background thread, no main queue dispatch.
static CVReturn displayLinkCallback(CVDisplayLinkRef displayLink,
    const CVTimeStamp *now, const CVTimeStamp *outputTime,
    CVOptionFlags flagsIn, CVOptionFlags *flagsOut, void *ctx) {
    ResizeView *view = (__bridge ResizeView *)ctx;
    if (!view.isResizing) {
        [view renderAsync];
    }
    return kCVReturnSuccess;
}

- (void)dealloc {
    if (_displayLink) {
        CVDisplayLinkStop(_displayLink);
        CVDisplayLinkRelease(_displayLink);
    }
}

// ---------------------------------------------------------------------------
// Resize Logic (Main Thread)
// ---------------------------------------------------------------------------
- (void)setFrameSize:(NSSize)newSize {
    [super setFrameSize:newSize];

    @synchronized (self) {
        _metalLayer.frame = self.layer.bounds;
        _overlayLayer.frame = self.layer.bounds;

        CGFloat scale = self.layer.contentsScale;
        _metalLayer.drawableSize = CGSizeMake(newSize.width * scale, newSize.height * scale);
        _metalLayer.contentsScale = scale;
        _overlayLayer.contentsScale = scale;
    }

    if (_isResizing) {
        _overlayLayer.hidden = NO;
        [self renderSyncToOverlay];

        // Reset idle timer — fires when mouse stops moving.
        [_resizeTimer invalidate];
        _resizeTimer = [NSTimer timerWithTimeInterval:_displayRefreshInterval
                                               target:self
                                             selector:@selector(resizeTimerFired)
                                             userInfo:nil
                                              repeats:YES];
        [[NSRunLoop currentRunLoop] addTimer:_resizeTimer forMode:NSRunLoopCommonModes];
    }
}

- (void)viewDidMoveToWindow {
    [super viewDidMoveToWindow];
    @synchronized (self) {
        CGFloat scale = self.layer.contentsScale;
        NSSize size = self.frame.size;
        _metalLayer.drawableSize = CGSizeMake(size.width * scale, size.height * scale);
    }
}

- (void)viewWillStartLiveResize {
    [super viewWillStartLiveResize];
    // Pause CVDisplayLink: the sync path in setFrameSize owns rendering now.
    // This avoids contention between background thread and main thread for
    // drawables and the command queue.
    CVDisplayLinkStop(_displayLink);
    _isResizing = YES;
}

- (void)resizeTimerFired {
    // Mouse has been still for one refresh interval during resize drag.
    // Switch to async mode: hide overlay, render via presentDrawable.
    // presentDrawable works here because Window Server only blocks layer
    // updates during ACTIVE geometry changes. Static mouse = no block.
    _overlayLayer.hidden = YES;
    _overlayLayer.contents = nil;
    [self renderAsync];
}

- (void)viewDidEndLiveResize {
    [super viewDidEndLiveResize];
    _isResizing = NO;
    _overlayLayer.hidden = YES;
    _overlayLayer.contents = nil;
    [_resizeTimer invalidate];
    _resizeTimer = nil;

    // Resume background rendering.
    CVDisplayLinkStart(_displayLink);
}

// ---------------------------------------------------------------------------
// SYNC RENDER (Main Thread — blocking, for flicker-free resize)
// ---------------------------------------------------------------------------
- (void)renderSyncToOverlay {
    id<CAMetalDrawable> drawable = [_metalLayer nextDrawable];
    if (!drawable) return;

    id<MTLCommandBuffer> cmdBuf = [_commandQueue commandBuffer];
    [self encodeFrame:cmdBuf drawable:drawable];

    // CRITICAL: Read IOSurface BEFORE present/commit.
    // After presentDrawable, accessing drawable.texture is undefined.
    // (We don't call presentDrawable here, but this ordering is still
    // required — the IOSurface reference must be captured while the
    // texture is still valid.)
    IOSurfaceRef ioSurface = drawable.texture.iosurface;

    // Do NOT call presentDrawable. During the resize tracking loop,
    // presented drawables never recycle (CA transactions don't commit).
    // After 2-3 presents, nextDrawable would block for 1 second.
    // Instead, commit + waitUntilCompleted ensures the GPU writes pixels
    // to the IOSurface, then we display it via the overlay layer.
    [cmdBuf commit];
    [cmdBuf waitUntilCompleted];

    if (ioSurface) {
        _overlayLayer.contents = (__bridge id)ioSurface;
    }
}

// ---------------------------------------------------------------------------
// ASYNC RENDER (Background Thread — non-blocking, double-buffered)
// ---------------------------------------------------------------------------
- (void)renderAsync {
    // Backpressure: block until a slot is free. This prevents the CPU from
    // queuing unbounded work, which would increase input latency.
    dispatch_semaphore_wait(_inFlightSemaphore, DISPATCH_TIME_FOREVER);

    @synchronized (self) {
        id<CAMetalDrawable> drawable = [_metalLayer nextDrawable];
        if (!drawable) {
            dispatch_semaphore_signal(_inFlightSemaphore);
            return;
        }

        id<MTLCommandBuffer> cmdBuf = [_commandQueue commandBuffer];

        __block dispatch_semaphore_t sema = _inFlightSemaphore;
        [cmdBuf addCompletedHandler:^(id<MTLCommandBuffer> buffer) {
            dispatch_semaphore_signal(sema);
        }];

        [self encodeFrame:cmdBuf drawable:drawable];

        [cmdBuf presentDrawable:drawable];
        [cmdBuf commit];
    }
}

// ---------------------------------------------------------------------------
// Shared encoding logic
// ---------------------------------------------------------------------------
- (void)encodeFrame:(id<MTLCommandBuffer>)cmdBuf drawable:(id<CAMetalDrawable>)drawable {
    MTLRenderPassDescriptor *rpd = [MTLRenderPassDescriptor renderPassDescriptor];
    rpd.colorAttachments[0].texture = drawable.texture;
    rpd.colorAttachments[0].loadAction = MTLLoadActionClear;
    rpd.colorAttachments[0].storeAction = MTLStoreActionStore;
    rpd.colorAttachments[0].clearColor = MTLClearColorMake(0.15, 0.15, 0.15, 1.0);

    id<MTLRenderCommandEncoder> enc = [cmdBuf renderCommandEncoderWithDescriptor:rpd];
    [enc setRenderPipelineState:_pipeline];
    float time = (float)(CACurrentMediaTime() - _startTime);
    [enc setFragmentBytes:&time length:sizeof(float) atIndex:0];
    [enc drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
    [enc endEncoding];
}

@end

// ---------------------------------------------------------------------------
// App Delegate & Main
// ---------------------------------------------------------------------------
@interface AppDelegate : NSObject <NSApplicationDelegate>
@property (nonatomic, strong) NSWindow *window;
@end

@implementation AppDelegate

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    NSRect frame = NSMakeRect(200, 200, 800, 600);
    _window = [[NSWindow alloc]
        initWithContentRect:frame
        styleMask:(NSWindowStyleMaskTitled |
                   NSWindowStyleMaskClosable |
                   NSWindowStyleMaskResizable |
                   NSWindowStyleMaskMiniaturizable)
        backing:NSBackingStoreBuffered
        defer:NO];
    _window.title = @"Ultra-Low Latency Metal Resize";
    _window.contentView = [[ResizeView alloc] initWithFrame:frame];
    _window.backgroundColor = [NSColor colorWithSRGBRed:0.15
                                                  green:0.15
                                                   blue:0.15
                                                  alpha:1.0];
    [_window makeKeyAndOrderFront:nil];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

@end

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        AppDelegate *delegate = [[AppDelegate alloc] init];
        app.delegate = delegate;
        [app activateIgnoringOtherApps:YES];
        [app run];
    }
    return 0;
}
