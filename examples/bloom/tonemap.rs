use crate::RenderContext;
use std::{mem, slice, sync::Arc};
use vulkano::{
    command_buffer::RenderPassBeginInfo,
    image::view::ImageView,
    pipeline::{
        graphics::{
            color_blend::{ColorBlendAttachmentState, ColorBlendState},
            input_assembly::InputAssemblyState,
            multisample::MultisampleState,
            rasterization::RasterizationState,
            vertex_input::VertexInputState,
            viewport::ViewportState,
            GraphicsPipelineCreateInfo,
        },
        DynamicState, GraphicsPipeline, PipelineBindPoint, PipelineShaderStageCreateInfo,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass},
    swapchain::Swapchain,
};
use vulkano_taskgraph::{
    command_buffer::RecordingCommandBuffer, Id, Task, TaskContext, TaskResult,
};

const EXPOSURE: f32 = 1.0;

pub struct TonemapTask {
    render_pass: Arc<RenderPass>,
    pipeline: Arc<GraphicsPipeline>,
    framebuffers: Vec<Arc<Framebuffer>>,
    swapchain_id: Id<Swapchain>,
}

impl TonemapTask {
    pub fn new(rcx: &RenderContext, swapchain_id: Id<Swapchain>) -> Self {
        let render_pass = vulkano::single_pass_renderpass!(
            rcx.device.clone(),
            attachments: {
                color: {
                    format: rcx.swapchain_format,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                },
            },
            pass: {
                color: [color],
                depth_stencil: {},
            },
        )
        .unwrap();

        let pipeline = {
            let vs = vs::load(rcx.device.clone())
                .unwrap()
                .entry_point("main")
                .unwrap();
            let fs = fs::load(rcx.device.clone())
                .unwrap()
                .entry_point("main")
                .unwrap();
            let stages = [
                PipelineShaderStageCreateInfo::new(vs),
                PipelineShaderStageCreateInfo::new(fs),
            ];
            let subpass = Subpass::from(render_pass.clone(), 0).unwrap();

            GraphicsPipeline::new(
                rcx.device.clone(),
                None,
                GraphicsPipelineCreateInfo {
                    stages: stages.into_iter().collect(),
                    vertex_input_state: Some(VertexInputState::default()),
                    input_assembly_state: Some(InputAssemblyState::default()),
                    viewport_state: Some(ViewportState::default()),
                    rasterization_state: Some(RasterizationState::default()),
                    multisample_state: Some(MultisampleState::default()),
                    color_blend_state: Some(ColorBlendState::with_attachment_states(
                        subpass.num_color_attachments(),
                        ColorBlendAttachmentState::default(),
                    )),
                    dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                    subpass: Some(subpass.into()),
                    ..GraphicsPipelineCreateInfo::layout(rcx.pipeline_layout.clone())
                },
            )
            .unwrap()
        };

        let framebuffers = window_size_dependent_setup(rcx, &render_pass);

        TonemapTask {
            render_pass,
            pipeline,
            framebuffers,
            swapchain_id,
        }
    }

    pub fn handle_resize(&mut self, rcx: &RenderContext) {
        let framebuffers = window_size_dependent_setup(rcx, &self.render_pass);

        let flight = rcx.resources.flight(rcx.flight_id).unwrap();
        flight.destroy_objects(mem::replace(&mut self.framebuffers, framebuffers));
    }

    pub fn cleanup(&mut self, rcx: &RenderContext) {
        let flight = rcx.resources.flight(rcx.flight_id).unwrap();
        flight.destroy_objects(self.framebuffers.drain(..));
    }
}

impl Task for TonemapTask {
    type World = RenderContext;

    unsafe fn execute(
        &self,
        cbf: &mut RecordingCommandBuffer<'_>,
        tcx: &mut TaskContext<'_>,
        rcx: &Self::World,
    ) -> TaskResult {
        cbf.as_raw().bind_descriptor_sets(
            PipelineBindPoint::Graphics,
            &rcx.pipeline_layout,
            0,
            slice::from_ref(&rcx.descriptor_set),
        )?;

        let swapchain_state = tcx.swapchain(self.swapchain_id)?;
        let image_index = swapchain_state.current_image_index().unwrap();

        cbf.as_raw().begin_render_pass(
            &RenderPassBeginInfo {
                clear_values: vec![None],
                ..RenderPassBeginInfo::framebuffer(self.framebuffers[image_index as usize].clone())
            },
            &Default::default(),
        )?;
        cbf.set_viewport(0, slice::from_ref(&rcx.viewport))?;
        cbf.bind_pipeline_graphics(&self.pipeline)?;
        cbf.push_constants(
            &rcx.pipeline_layout,
            0,
            &fs::PushConstants { exposure: EXPOSURE },
        )?;

        unsafe { cbf.draw(3, 1, 0, 0) }?;

        cbf.as_raw().end_render_pass(&Default::default())?;

        Ok(())
    }
}

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        src: r"
            #version 450

            const vec2[3] POSITIONS = {
                vec2(-1.0, -1.0),
                vec2(-1.0,  3.0),
                vec2( 3.0, -1.0),
            };

            const vec2[3] TEX_COORDS = {
                vec2(0.0, 0.0),
                vec2(0.0, 2.0),
                vec2(2.0, 0.0),
            };

            layout(location = 0) out vec2 v_tex_coords;

            void main() {
                gl_Position = vec4(POSITIONS[gl_VertexIndex], 0.0, 1.0);
                v_tex_coords = TEX_COORDS[gl_VertexIndex];
            }
        ",
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "tonemap.glsl",
        include: ["."],
    }
}

fn window_size_dependent_setup(
    rcx: &RenderContext,
    render_pass: &Arc<RenderPass>,
) -> Vec<Arc<Framebuffer>> {
    let swapchain_state = rcx.resources.swapchain(rcx.swapchain_id).unwrap();
    let images = swapchain_state.images();

    images
        .iter()
        .map(|image| {
            let view = ImageView::new_default(image.clone()).unwrap();
            Framebuffer::new(
                render_pass.clone(),
                FramebufferCreateInfo {
                    attachments: vec![view],
                    ..Default::default()
                },
            )
            .unwrap()
        })
        .collect::<Vec<_>>()
}