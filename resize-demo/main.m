// Ultra-Low Latency macOS Metal Resize Demo
// Features:
// 1. IOSurface Sync for flicker-free resize.
// 2. Background Thread Rendering for 120Hz smoothness.
// 3. Triple Buffering (Semaphore) for lowest input latency.
//
// Build: clang -framework Cocoa -framework Metal -framework QuartzCore
//        -framework IOSurface -framework CoreVideo -o resize-demo main.m
// Run:   ./resize-demo

#import <Cocoa/Cocoa.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#import <IOSurface/IOSurface.h>

static const NSUInteger kMaxInFlightFrameCount = 3;

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

        // CVDisplayLink — renders on background thread.
        CVDisplayLinkCreateWithActiveCGDisplays(&_displayLink);
        CVDisplayLinkSetOutputCallback(_displayLink, &displayLinkCallback, (__bridge void *)self);
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
        _resizeTimer = [NSTimer timerWithTimeInterval:1.0/120.0
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
    // Pause background rendering to avoid contention.
    CVDisplayLinkStop(_displayLink);
    _isResizing = YES;
}

- (void)resizeTimerFired {
    // Mouse stopped — hide overlay, render via presentDrawable.
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

    IOSurfaceRef ioSurface = drawable.texture.iosurface;

    [cmdBuf commit];
    [cmdBuf waitUntilCompleted];

    if (ioSurface) {
        _overlayLayer.contents = (__bridge id)ioSurface;
    }
}

// ---------------------------------------------------------------------------
// ASYNC RENDER (Background Thread — non-blocking, triple-buffered)
// ---------------------------------------------------------------------------
- (void)renderAsync {
    // Wait for an available slot (triple buffering backpressure).
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
