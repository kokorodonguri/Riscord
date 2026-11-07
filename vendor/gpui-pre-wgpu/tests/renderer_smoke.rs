#[cfg(test)]
mod tests {
    use gpui::{BorderStyle, Bounds, ContentMask, Corners, DevicePixels, Edges, Hsla, PlatformHeadlessRenderer, Point, Quad, Scene, Size, Underline, PrimitiveBatch, AtlasKey, ImageId, RenderImageParams, PolychromeSprite, RenderSvgParams, MonochromeSprite, TransformationMatrix};
    fn quad(x: f32, color: Hsla) -> Quad {
        let bounds = Bounds { origin: Point { x: x.into(), y: 0.0.into() }, size: Size { width: 32.0.into(), height: 32.0.into() } };
        Quad { order: 0, border_style: BorderStyle::Solid, bounds, content_mask: ContentMask { bounds }, background: color.into(), border_color: color, corner_radii: Corners::default(), border_widths: Edges::default() }
    }
    fn assert_pixel(image: &image::RgbaImage, x: u32, y: u32, expected: [u8; 4]) {
        let actual = image.get_pixel(x, y).0;
        assert!(actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 3), "pixel ({x}, {y}): {actual:?}, expected {expected:?}");
    }
    #[test]
    fn draws_distinct_colors() -> anyhow::Result<()> {
        let mut renderer = gpui_wgpu::WgpuHeadlessRenderer::new()?;
        let mut scene = Scene::default();
        scene.insert_primitive(quad(0.0, gpui::red()));
        scene.insert_primitive(quad(32.0, gpui::blue()));
        scene.finish();
        let image = renderer.render_scene_to_image(&scene, Size { width: DevicePixels(64), height: DevicePixels(32) })?;
        assert_pixel(&image, 16, 16, [255, 0, 0, 255]);
        assert_pixel(&image, 48, 16, [0, 0, 255, 255]);
        Ok(())
    }
    #[test]
    fn separated_quad_batches_preserve_layering() -> anyhow::Result<()> {
        let mut renderer = gpui_wgpu::WgpuHeadlessRenderer::new()?;
        let mut scene = Scene::default();
        let mut base = quad(0.0, gpui::red());
        base.bounds.size.width = 64.0.into();
        base.content_mask.bounds = base.bounds;
        scene.insert_primitive(base);
        let bounds = Bounds { origin: Point { x: 0.0.into(), y: 4.0.into() }, size: Size { width: 64.0.into(), height: 4.0.into() } };
        scene.insert_primitive(Underline { order: 0, pad: 0, bounds, content_mask: ContentMask { bounds }, color: gpui::white(), thickness: 4.0.into(), wavy: false.into() });
        scene.insert_primitive(quad(0.0, gpui::blue()));
        scene.finish();
        assert_eq!(scene.batches().filter(|b| matches!(b, PrimitiveBatch::Quads(_))).count(), 2);
        let image = renderer.render_scene_to_image(&scene, Size { width: DevicePixels(64), height: DevicePixels(32) })?;
        assert_pixel(&image, 16, 16, [0, 0, 255, 255]);
        assert_pixel(&image, 48, 16, [255, 0, 0, 255]);
        Ok(())
    }

    #[test]
    fn sprite_batch_between_quads_is_rendered() -> anyhow::Result<()> {
        let mut renderer = gpui_wgpu::WgpuHeadlessRenderer::new()?;
        let atlas = renderer.sprite_atlas();
        let key = AtlasKey::Image(RenderImageParams { image_id: ImageId(123456), frame_index: 0 });
        let tile = atlas.get_or_insert_with(key, &mut || Ok(Some((Size { width: DevicePixels(8), height: DevicePixels(8) }, std::borrow::Cow::Owned([0, 0, 255, 255].repeat(64))))))?.unwrap();
        let mut scene = Scene::default();
        let mut base = quad(0.0, gpui::blue());
        base.bounds.size.width = 64.0.into();
        base.content_mask.bounds = base.bounds;
        scene.insert_primitive(base);
        let bounds = Bounds { origin: Point { x: 16.0.into(), y: 0.0.into() }, size: Size { width: 32.0.into(), height: 32.0.into() } };
        scene.insert_primitive(PolychromeSprite { order: 0, pad: 0, tile, grayscale: false.into(), opacity: 1.0, bounds, content_mask: ContentMask { bounds }, corner_radii: Corners::default() });
        let mut top = quad(32.0, gpui::blue());
        top.bounds.size.width = 8.0.into();
        top.content_mask.bounds = top.bounds;
        scene.insert_primitive(top);
        scene.finish();
        assert_eq!(scene.batches().filter(|b| matches!(b, PrimitiveBatch::Quads(_))).count(), 2);
        let image = renderer.render_scene_to_image(&scene, Size { width: DevicePixels(64), height: DevicePixels(32) })?;
        assert_pixel(&image, 8, 16, [0, 0, 255, 255]);
        assert_pixel(&image, 24, 16, [255, 0, 0, 255]);
        assert_pixel(&image, 36, 16, [0, 0, 255, 255]);
        Ok(())
    }

    #[test]
    fn separated_monochrome_batches_render_on_top() -> anyhow::Result<()> {
        let mut renderer = gpui_wgpu::WgpuHeadlessRenderer::new()?;
        let atlas = renderer.sprite_atlas();
        let size = Size { width: DevicePixels(8), height: DevicePixels(8) };
        let key = AtlasKey::Svg(RenderSvgParams { path: "test-glyph".into(), size });
        let tile = atlas.get_or_insert_with(key, &mut || Ok(Some((size, std::borrow::Cow::Owned(vec![255; 64])))))?.unwrap();
        let mut scene = Scene::default();
        let mut base = quad(0.0, gpui::blue());
        base.bounds.size.width = 64.0.into();
        base.content_mask.bounds = base.bounds;
        scene.insert_primitive(base);
        let left = Bounds { origin: Point { x: 0.0.into(), y: 0.0.into() }, size: Size { width: 32.0.into(), height: 32.0.into() } };
        let sprite = |bounds| MonochromeSprite { order: 0, pad: 0, tile, bounds, content_mask: ContentMask { bounds }, color: gpui::white(), transformation: TransformationMatrix::default() };
        scene.insert_primitive(sprite(left));
        let mut cover = quad(0.0, gpui::red());
        cover.bounds.size.width = 64.0.into();
        cover.content_mask.bounds = cover.bounds;
        scene.insert_primitive(cover);
        let right = Bounds { origin: Point { x: 32.0.into(), y: 0.0.into() }, size: Size { width: 32.0.into(), height: 32.0.into() } };
        scene.insert_primitive(sprite(right));
        scene.finish();
        assert_eq!(scene.batches().filter(|b| matches!(b, PrimitiveBatch::MonochromeSprites { .. })).count(), 2);
        let image = renderer.render_scene_to_image(&scene, Size { width: DevicePixels(64), height: DevicePixels(32) })?;
        assert_pixel(&image, 16, 16, [255, 0, 0, 255]);
        assert_pixel(&image, 48, 16, [255, 255, 255, 255]);
        Ok(())
    }

}
