// Flocking boids example with gpu compute update pass
// adapted from https://github.com/austinEng/webgpu-samples/blob/master/src/examples/computeBoids.ts

#[path = "../framework.rs"]
mod framework;

// number of boid particles to simulate

const NUM_PARTICLES: u32 = 1500;

// number of single-particle calculations (invocations) in each gpu work group

const PARTICLES_PER_GROUP: u32 = 64;

/// Example struct holds references to wgpu resources and frame persistent data
struct Example {
    particle_bind_groups: Vec<wgpu::BindGroup>,
    particle_buffers: Vec<wgpu::Buffer>,
    vertices_buffer: wgpu::Buffer,
    compute_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    work_group_count: u32,
    frame_num: usize,
}

impl framework::Example for Example {
    /// constructs initial instance of Example struct
    fn init(
        sc_desc: &wgpu::SwapChainDescriptor,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> Self {
        // load (and compile) shaders and create shader modules

        let boids_module = device.create_shader_module(wgpu::include_spirv!("boids.comp.spv"));
        let vs_module = device.create_shader_module(wgpu::include_spirv!("shader.vert.spv"));
        let fs_module = device.create_shader_module(wgpu::include_spirv!("shader.frag.spv"));

        // buffer for simulation parameters uniform

        let sim_param_data = [
            0.04f32, // deltaT
            0.1,     // rule1Distance
            0.025,   // rule2Distance
            0.025,   // rule3Distance
            0.02,    // rule1Scale
            0.05,    // rule2Scale
            0.005,   // rule3Scale
        ]
        .to_vec();
        let sim_param_buffer = device.create_buffer_with_data(
            bytemuck::cast_slice(&sim_param_data),
            wgpu::BufferUsage::UNIFORM | wgpu::BufferUsage::COPY_DST,
        );

        // create compute bind layout group and compute pipeline layout

        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                bindings: &[
                    wgpu::BindGroupLayoutEntry::new(
                        0,
                        wgpu::ShaderStage::COMPUTE,
                        wgpu::BindingType::UniformBuffer {
                            dynamic: false,
                            min_binding_size: wgpu::BufferSize::new(sim_param_data.len() as _),
                        },
                    ),
                    wgpu::BindGroupLayoutEntry::new(
                        1,
                        wgpu::ShaderStage::COMPUTE,
                        wgpu::BindingType::StorageBuffer {
                            dynamic: false,
                            min_binding_size: wgpu::BufferSize::new((NUM_PARTICLES * 16) as _),
                            readonly: false,
                        },
                    ),
                    wgpu::BindGroupLayoutEntry::new(
                        2,
                        wgpu::ShaderStage::COMPUTE,
                        wgpu::BindingType::StorageBuffer {
                            dynamic: false,
                            min_binding_size: wgpu::BufferSize::new((NUM_PARTICLES * 16) as _),
                            readonly: false,
                        },
                    ),
                ],
                label: None,
            });
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[&compute_bind_group_layout],
            });

        // create render pipeline with empty bind group layout

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[],
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            layout: &render_pipeline_layout,
            vertex_stage: wgpu::ProgrammableStageDescriptor {
                module: &vs_module,
                entry_point: "main",
            },
            fragment_stage: Some(wgpu::ProgrammableStageDescriptor {
                module: &fs_module,
                entry_point: "main",
            }),
            rasterization_state: Some(wgpu::RasterizationStateDescriptor {
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: wgpu::CullMode::None,
                depth_bias: 0,
                depth_bias_slope_scale: 0.0,
                depth_bias_clamp: 0.0,
            }),
            primitive_topology: wgpu::PrimitiveTopology::TriangleList,
            color_states: &[wgpu::ColorStateDescriptor {
                format: sc_desc.format,
                color_blend: wgpu::BlendDescriptor::REPLACE,
                alpha_blend: wgpu::BlendDescriptor::REPLACE,
                write_mask: wgpu::ColorWrite::ALL,
            }],
            depth_stencil_state: None,
            vertex_state: wgpu::VertexStateDescriptor {
                index_format: wgpu::IndexFormat::Uint16,
                vertex_buffers: &[
                    wgpu::VertexBufferDescriptor {
                        stride: 4 * 4,
                        step_mode: wgpu::InputStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float2, 1 => Float2],
                    },
                    wgpu::VertexBufferDescriptor {
                        stride: 2 * 4,
                        step_mode: wgpu::InputStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![2 => Float2],
                    },
                ],
            },
            sample_count: 1,
            sample_mask: !0,
            alpha_to_coverage_enabled: false,
        });

        // create compute pipeline

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            layout: &compute_pipeline_layout,
            compute_stage: wgpu::ProgrammableStageDescriptor {
                module: &boids_module,
                entry_point: "main",
            },
        });

        // buffer for the three 2d triangle vertices of each instance

        let vertex_buffer_data = [-0.01f32, -0.02, 0.01, -0.02, 0.00, 0.02];
        let vertices_buffer = device.create_buffer_with_data(
            bytemuck::bytes_of(&vertex_buffer_data),
            wgpu::BufferUsage::VERTEX | wgpu::BufferUsage::COPY_DST,
        );

        // buffer for all particles data of type [(posx,posy,velx,vely),...]

        let mut initial_particle_data = vec![0.0f32; (4 * NUM_PARTICLES) as usize];
        for particle_instance_chunk in initial_particle_data.chunks_mut(4) {
            particle_instance_chunk[0] = 2.0 * (rand::random::<f32>() - 0.5); // posx
            particle_instance_chunk[1] = 2.0 * (rand::random::<f32>() - 0.5); // posy
            particle_instance_chunk[2] = 2.0 * (rand::random::<f32>() - 0.5) * 0.1; // velx
            particle_instance_chunk[3] = 2.0 * (rand::random::<f32>() - 0.5) * 0.1;
            // vely
        }

        // creates two buffers of particle data each of size NUM_PARTICLES
        // the two buffers alternate as dst and src for each frame

        let mut particle_buffers = Vec::<wgpu::Buffer>::new();
        let mut particle_bind_groups = Vec::<wgpu::BindGroup>::new();
        for _i in 0..2 {
            particle_buffers.push(device.create_buffer_with_data(
                bytemuck::cast_slice(&initial_particle_data),
                wgpu::BufferUsage::VERTEX
                    | wgpu::BufferUsage::STORAGE
                    | wgpu::BufferUsage::COPY_DST,
            ));
        }

        // create two bind groups, one for each buffer as the src
        // where the alternate buffer is used as the dst

        for i in 0..2 {
            particle_bind_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &compute_bind_group_layout,
                bindings: &[
                    wgpu::Binding {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(sim_param_buffer.slice(..)),
                    },
                    wgpu::Binding {
                        binding: 1,
                        resource: wgpu::BindingResource::Buffer(particle_buffers[i].slice(..)),
                    },
                    wgpu::Binding {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(
                            particle_buffers[(i + 1) % 2].slice(..), // bind to opposite buffer
                        ),
                    },
                ],
                label: None,
            }));
        }

        // calculates number of work groups from PARTICLES_PER_GROUP constant
        let work_group_count =
            ((NUM_PARTICLES as f32) / (PARTICLES_PER_GROUP as f32)).ceil() as u32;

        // returns Example struct and No encoder commands

        Example {
            particle_bind_groups,
            particle_buffers,
            vertices_buffer,
            compute_pipeline,
            render_pipeline,
            work_group_count,
            frame_num: 0,
        }
    }

    /// update is called for any WindowEvent not handled by the framework
    fn update(&mut self, _event: winit::event::WindowEvent) {
        //empty
    }

    /// resize is called on WindowEvent::Resized events
    fn resize(
        &mut self,
        _sc_desc: &wgpu::SwapChainDescriptor,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) {
        //empty
    }

    /// render is called each frame, dispatching compute groups proportional
    ///   a TriangleList draw call for all NUM_PARTICLES at 3 vertices each
    fn render(
        &mut self,
        frame: &wgpu::SwapChainTexture,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _spawner: &impl futures::task::LocalSpawn,
    ) {
        // create render pass descriptor
        let render_pass_descriptor = wgpu::RenderPassDescriptor {
            color_attachments: &[wgpu::RenderPassColorAttachmentDescriptor {
                attachment: &frame.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: true,
                },
            }],
            depth_stencil_attachment: None,
        };

        // get command encoder
        let mut command_encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        {
            // compute pass
            let mut cpass = command_encoder.begin_compute_pass();
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, &self.particle_bind_groups[self.frame_num % 2], &[]);
            cpass.dispatch(self.work_group_count, 1, 1);
        }

        {
            // render pass
            let mut rpass = command_encoder.begin_render_pass(&render_pass_descriptor);
            rpass.set_pipeline(&self.render_pipeline);
            // render dst particles
            rpass.set_vertex_buffer(0, self.particle_buffers[(self.frame_num + 1) % 2].slice(..));
            // the three instance-local vertices
            rpass.set_vertex_buffer(1, self.vertices_buffer.slice(..));
            rpass.draw(0..3, 0..NUM_PARTICLES);
        }

        // update frame count
        self.frame_num += 1;

        // done
        queue.submit(Some(command_encoder.finish()));
    }
}

/// run example
fn main() {
    framework::run::<Example>("boids");
}
