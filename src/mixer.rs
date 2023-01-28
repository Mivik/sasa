use ringbuf::HeapConsumer;
use crate::Renderer;

pub(crate) enum MixerCommand {
    AddRenderer(Box<dyn Renderer>),
}
pub(crate) struct Mixer {
    pub(crate) sample_rate: u32,

    renderers: Vec<Box<dyn Renderer>>,
    cons: HeapConsumer<MixerCommand>,
}

impl Mixer {
    pub(crate) fn new(sample_rate: u32, cons: HeapConsumer<MixerCommand>) -> Self {
        Self {
            sample_rate,

            renderers: Vec::new(),
            cons,
        }
    }

    fn consume_commands(&mut self) {
        for cmd in self.cons.pop_iter() {
            match cmd {
                MixerCommand::AddRenderer(renderer) => self.renderers.push(renderer),
            }
        }
    }

    pub fn render_mono(&mut self, data: &mut [f32]) {
        self.consume_commands();
        data.fill(0.);

        self.renderers.retain_mut(|renderer| {
            renderer.render_mono(self.sample_rate, data);
            renderer.alive()
        });
    }

    pub fn render_stereo(&mut self, data: &mut [f32]) {
        self.consume_commands();
        data.fill(0.);

        self.renderers.retain_mut(|renderer| {
            renderer.render_stereo(self.sample_rate, data);
            renderer.alive()
        });
    }
}