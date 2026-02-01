use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer as GlyphonTextRenderer, Viewport,
};

pub struct TextEntry {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub color: [u8; 4],
    pub font_size: f32,
}

pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    #[allow(dead_code)]
    cache: Cache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderer: GlyphonTextRenderer,
    buffers: Vec<Buffer>,
}

impl TextRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, surface_format: wgpu::TextureFormat) -> Self {
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, surface_format);
        let viewport = Viewport::new(device, &cache);
        let renderer = GlyphonTextRenderer::new(
            &mut atlas,
            device,
            wgpu::MultisampleState::default(),
            None, // no depth stencil for text overlay
        );

        Self {
            font_system,
            swash_cache,
            cache,
            atlas,
            viewport,
            renderer,
            buffers: Vec::new(),
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        entries: &[TextEntry],
    ) {
        self.viewport.update(queue, Resolution { width, height });

        // Rebuild buffers for each entry
        self.buffers.clear();
        for entry in entries {
            let metrics = Metrics::new(entry.font_size, entry.font_size * 1.2);
            let mut buffer = Buffer::new(&mut self.font_system, metrics);
            let attrs = Attrs::new().family(Family::Name("Envy Code R"));
            buffer.set_text(&mut self.font_system, &entry.text, &attrs, Shaping::Basic, None);
            buffer.set_size(&mut self.font_system, Some(width as f32), Some(height as f32));
            buffer.shape_until_scroll(&mut self.font_system, false);
            self.buffers.push(buffer);
        }

        let text_areas: Vec<TextArea> = entries
            .iter()
            .zip(self.buffers.iter())
            .map(|(entry, buffer)| TextArea {
                buffer,
                left: entry.x,
                top: entry.y,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: width as i32,
                    bottom: height as i32,
                },
                default_color: Color::rgba(
                    entry.color[0],
                    entry.color[1],
                    entry.color[2],
                    entry.color[3],
                ),
                custom_glyphs: &[],
            })
            .collect();

        self.atlas.trim();

        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .expect("Failed to prepare text");
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .expect("Failed to render text");
    }
}
